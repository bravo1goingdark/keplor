//! [`BatchWriter`] — async event writer that accumulates events and flushes
//! them in bulk transactions for high throughput.

use std::sync::Arc;
use std::time::Duration;

use bytes::Bytes;
use tokio::sync::mpsc;

use keplor_core::{EventId, LlmEvent};

use crate::error::StoreError;
use crate::store::Store;

/// Configuration for the [`BatchWriter`].
#[derive(Debug, Clone)]
pub struct BatchConfig {
    /// Maximum events to accumulate before forcing a flush.
    pub batch_size: usize,
    /// Maximum time to wait before flushing a partial batch.
    pub flush_interval: Duration,
    /// Bounded channel capacity for back-pressure.
    pub channel_capacity: usize,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self { batch_size: 256, flush_interval: Duration::from_millis(50), channel_capacity: 8192 }
    }
}

/// Async event writer that batches writes for throughput.
///
/// Callers send events via [`BatchWriter::write`]. A background task
/// accumulates them and flushes in bulk transactions, amortising
/// `BEGIN`/`COMMIT` and prepared-statement overhead across many events.
///
/// On shutdown, call [`BatchWriter::shutdown`] to drain all pending
/// events before the process exits.
pub struct BatchWriter {
    tx: std::sync::Mutex<Option<mpsc::Sender<WriteRequest>>>,
    channel_capacity: usize,
    flush_handle: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

struct WriteRequest {
    event: LlmEvent,
    req_body: Bytes,
    resp_body: Bytes,
    result_tx: Option<tokio::sync::oneshot::Sender<Result<EventId, StoreError>>>,
}

impl BatchWriter {
    /// Spawn a new batch writer backed by `store`.
    ///
    /// The background flush task runs until [`BatchWriter::shutdown`] is
    /// called or all senders are dropped.
    pub fn new(store: Arc<Store>, config: BatchConfig) -> Self {
        let capacity = config.channel_capacity;
        let (tx, rx) = mpsc::channel(capacity);
        let flush_handle = tokio::spawn(flush_loop(store, rx, config));
        Self {
            tx: std::sync::Mutex::new(Some(tx)),
            channel_capacity: capacity,
            flush_handle: tokio::sync::Mutex::new(Some(flush_handle)),
        }
    }

    /// Submit an event for batched writing.
    ///
    /// Returns once the event is durably flushed.
    pub async fn write(
        &self,
        event: LlmEvent,
        req_body: Bytes,
        resp_body: Bytes,
    ) -> Result<EventId, StoreError> {
        let tx = {
            let guard = self.tx.lock().unwrap_or_else(|e| e.into_inner());
            guard.clone().ok_or(StoreError::ChannelClosed)?
        };
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        tx.send(WriteRequest { event, req_body, resp_body, result_tx: Some(result_tx) })
            .await
            .map_err(|_| StoreError::ChannelClosed)?;
        result_rx.await.map_err(|_| StoreError::ChannelClosed)?
    }

    /// Submit an event without waiting for the flush.
    pub fn write_fire_and_forget(
        &self,
        event: LlmEvent,
        req_body: Bytes,
        resp_body: Bytes,
    ) -> Result<(), StoreError> {
        let guard = self.tx.lock().unwrap_or_else(|e| e.into_inner());
        let tx = guard.as_ref().ok_or(StoreError::ChannelClosed)?;
        tx.try_send(WriteRequest { event, req_body, resp_body, result_tx: None }).map_err(|e| {
            match e {
                mpsc::error::TrySendError::Full(_) => StoreError::ChannelFull,
                mpsc::error::TrySendError::Closed(_) => StoreError::ChannelClosed,
            }
        })
    }

    /// Number of events currently queued in the channel.
    ///
    /// Returns 0 if the channel is closed.
    pub fn queue_depth(&self) -> usize {
        let guard = self.tx.lock().unwrap_or_else(|e| e.into_inner());
        guard.as_ref().map(|tx| tx.max_capacity() - tx.capacity()).unwrap_or(0)
    }

    /// Maximum channel capacity.
    pub fn max_capacity(&self) -> usize {
        self.channel_capacity
    }

    /// Gracefully shut down the batch writer.
    ///
    /// Drops the sender to close the channel, then waits for the flush
    /// loop to drain all pending events up to `timeout`.
    ///
    /// Returns `true` if the flush loop completed within the deadline,
    /// `false` if the timeout expired (some events may be lost).
    pub async fn shutdown(&self, timeout: Duration) -> bool {
        // Drop the sender to close the channel — the flush loop will
        // see `None` from `rx.recv()` and drain remaining events.
        {
            let mut guard = self.tx.lock().unwrap_or_else(|e| e.into_inner());
            guard.take();
        }

        // Take and await the flush handle.
        let handle = self.flush_handle.lock().await.take();
        if let Some(handle) = handle {
            tokio::time::timeout(timeout, handle).await.is_ok()
        } else {
            true
        }
    }
}

async fn flush_loop(store: Arc<Store>, mut rx: mpsc::Receiver<WriteRequest>, config: BatchConfig) {
    let mut buffer: Vec<WriteRequest> = Vec::with_capacity(config.batch_size);
    let mut interval = tokio::time::interval(config.flush_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    loop {
        tokio::select! {
            biased;
            req = rx.recv() => {
                match req {
                    Some(r) => {
                        buffer.push(r);
                        while buffer.len() < config.batch_size {
                            match rx.try_recv() {
                                Ok(r) => buffer.push(r),
                                Err(_) => break,
                            }
                        }
                        if buffer.len() >= config.batch_size {
                            flush(&store, &mut buffer).await;
                        }
                    },
                    None => {
                        // Channel closed — drain remaining events.
                        if !buffer.is_empty() {
                            tracing::info!(events = buffer.len(), "draining batch writer on shutdown");
                            flush(&store, &mut buffer).await;
                        }
                        tracing::info!("batch writer shut down cleanly");
                        return;
                    },
                }
            },
            _ = interval.tick() => {
                if !buffer.is_empty() {
                    flush(&store, &mut buffer).await;
                }
            },
        }
    }
}

/// Flush buffered writes via `spawn_blocking` so the synchronous SQLite
/// I/O never blocks a tokio worker thread.
async fn flush(store: &Arc<Store>, buffer: &mut Vec<WriteRequest>) {
    let pending = std::mem::take(buffer);
    let count = pending.len();

    let mut senders: Vec<Option<tokio::sync::oneshot::Sender<Result<EventId, StoreError>>>> =
        Vec::with_capacity(pending.len());
    let mut batch: Vec<(LlmEvent, Bytes, Bytes)> = Vec::with_capacity(pending.len());

    for req in pending {
        senders.push(req.result_tx);
        batch.push((req.event, req.req_body, req.resp_body));
    }

    let store = Arc::clone(store);
    let result = tokio::task::spawn_blocking(move || store.append_batch(&batch))
        .await
        .unwrap_or_else(|e| Err(StoreError::Compression(e.to_string())));

    match result {
        Ok(ids) => {
            metrics::counter!("keplor_batch_flushes_total").increment(1);
            metrics::counter!("keplor_batch_events_flushed_total").increment(count as u64);
            for (tx, id) in senders.into_iter().zip(ids) {
                if let Some(tx) = tx {
                    let _ = tx.send(Ok(id));
                }
            }
        },
        Err(e) => {
            metrics::counter!("keplor_batch_flush_errors_total").increment(1);
            let msg = e.to_string();
            tracing::error!(events = count, error = %msg, "batch flush failed");
            for tx in senders.into_iter().flatten() {
                let _ = tx.send(Err(StoreError::Compression(msg.clone())));
            }
        },
    }
}
