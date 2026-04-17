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
    tx: mpsc::Sender<WriteRequest>,
    flush_handle: Option<tokio::task::JoinHandle<()>>,
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
        let (tx, rx) = mpsc::channel(config.channel_capacity);
        let flush_handle = tokio::spawn(flush_loop(store, rx, config));
        Self { tx, flush_handle: Some(flush_handle) }
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
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(WriteRequest { event, req_body, resp_body, result_tx: Some(result_tx) })
            .await
            .map_err(|_| StoreError::Compression("batch writer channel closed".into()))?;
        result_rx.await.map_err(|_| StoreError::Compression("batch writer dropped".into()))?
    }

    /// Submit an event without waiting for the flush.
    pub fn write_fire_and_forget(
        &self,
        event: LlmEvent,
        req_body: Bytes,
        resp_body: Bytes,
    ) -> Result<(), StoreError> {
        self.tx
            .try_send(WriteRequest { event, req_body, resp_body, result_tx: None })
            .map_err(|_| StoreError::Compression("batch writer channel full".into()))
    }

    /// Wait for the flush loop to finish after the channel is closed.
    ///
    /// The channel closes when all `Sender` clones are dropped. The
    /// flush loop will drain remaining events and exit.
    pub async fn closed(&self) {
        self.tx.closed().await;
    }
}

impl Drop for BatchWriter {
    fn drop(&mut self) {
        // When the BatchWriter is dropped, the tx Sender is also dropped,
        // which closes the channel and causes the flush_loop to drain.
        // The JoinHandle is dropped too — the flush task will finish in
        // the background if the runtime is still alive.
        drop(self.flush_handle.take());
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
