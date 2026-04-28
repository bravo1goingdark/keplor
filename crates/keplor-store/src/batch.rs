//! [`BatchWriter`] — async event writer that accumulates events and flushes
//! them in bulk transactions for high throughput.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use tokio::sync::mpsc;

use keplor_core::{EventId, LlmEvent};

use crate::error::StoreError;
use crate::kdb_store::KdbStore;

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
    /// Channel sender — cloneable, no mutex needed for send operations.
    tx: mpsc::Sender<WriteRequest>,
    /// Set to `true` after shutdown to reject new writes fast.
    shutdown: AtomicBool,
    channel_capacity: usize,
    flush_handle: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

struct WriteRequest {
    event: LlmEvent,
    result_tx: Option<tokio::sync::oneshot::Sender<Result<EventId, StoreError>>>,
}

impl BatchWriter {
    /// Spawn a new batch writer backed by `store`.
    ///
    /// The background flush task runs until [`BatchWriter::shutdown`] is
    /// called or all senders are dropped.
    pub fn new(store: Arc<KdbStore>, config: BatchConfig) -> Self {
        let capacity = config.channel_capacity;
        let (tx, rx) = mpsc::channel(capacity);
        let flush_handle = tokio::spawn(flush_loop(store, rx, config));
        Self {
            tx,
            shutdown: AtomicBool::new(false),
            channel_capacity: capacity,
            flush_handle: tokio::sync::Mutex::new(Some(flush_handle)),
        }
    }

    /// Submit an event for batched writing.
    ///
    /// Returns once the event is durably flushed.
    pub async fn write(&self, event: LlmEvent) -> Result<EventId, StoreError> {
        if self.shutdown.load(Ordering::Relaxed) {
            return Err(StoreError::ChannelClosed);
        }
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        self.tx
            .send(WriteRequest { event, result_tx: Some(result_tx) })
            .await
            .map_err(|_| StoreError::ChannelClosed)?;
        result_rx.await.map_err(|_| StoreError::ChannelClosed)?
    }

    /// Submit multiple events and await all their flush confirmations.
    ///
    /// All events are sent to the channel first, then all oneshot
    /// receivers are awaited concurrently.  This avoids the serial-await
    /// problem where each event waits for a separate flush cycle.
    pub async fn write_many(&self, events: Vec<LlmEvent>) -> Vec<Result<EventId, StoreError>> {
        if self.shutdown.load(Ordering::Relaxed) {
            return events.iter().map(|_| Err(StoreError::ChannelClosed)).collect();
        }

        let mut receivers = Vec::with_capacity(events.len());
        for event in events {
            let (result_tx, result_rx) = tokio::sync::oneshot::channel();
            if self.tx.send(WriteRequest { event, result_tx: Some(result_tx) }).await.is_err() {
                receivers.push(None);
            } else {
                receivers.push(Some(result_rx));
            }
        }

        let mut results = Vec::with_capacity(receivers.len());
        for rx in receivers {
            match rx {
                Some(rx) => {
                    results.push(rx.await.unwrap_or(Err(StoreError::ChannelClosed)));
                },
                None => results.push(Err(StoreError::ChannelClosed)),
            }
        }
        results
    }

    /// Submit an event without waiting for the flush.
    pub fn write_fire_and_forget(&self, event: LlmEvent) -> Result<(), StoreError> {
        if self.shutdown.load(Ordering::Relaxed) {
            return Err(StoreError::ChannelClosed);
        }
        self.tx.try_send(WriteRequest { event, result_tx: None }).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => StoreError::ChannelFull,
            mpsc::error::TrySendError::Closed(_) => StoreError::ChannelClosed,
        })
    }

    /// Number of events currently queued in the channel.
    ///
    /// Returns 0 if the channel is closed.
    pub fn queue_depth(&self) -> usize {
        self.tx.max_capacity() - self.tx.capacity()
    }

    /// Maximum channel capacity.
    pub fn max_capacity(&self) -> usize {
        self.channel_capacity
    }

    /// Gracefully shut down the batch writer.
    ///
    /// Sets the shutdown flag to reject new writes, then waits for the
    /// flush loop to drain all pending events up to `timeout`.
    ///
    /// Returns `true` if the flush loop completed within the deadline,
    /// `false` if the timeout expired (some events may be lost).
    pub async fn shutdown(&self, timeout: Duration) -> bool {
        // Signal writers to stop — new write/write_fire_and_forget calls
        // will return ChannelClosed immediately.
        self.shutdown.store(true, Ordering::Relaxed);

        // Take and await the flush handle. The channel stays open until
        // all senders are dropped (including the one in this struct),
        // which happens when BatchWriter is dropped. The flush loop
        // drains remaining buffered events when it sees the channel close.
        let handle = self.flush_handle.lock().await.take();
        if let Some(handle) = handle {
            tokio::time::timeout(timeout, handle).await.is_ok()
        } else {
            true
        }
    }
}

async fn flush_loop(
    store: Arc<KdbStore>,
    mut rx: mpsc::Receiver<WriteRequest>,
    config: BatchConfig,
) {
    let mut buffer: Vec<WriteRequest> = Vec::with_capacity(config.batch_size);
    let mut interval = tokio::time::interval(config.flush_interval);
    interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

    let capacity = config.channel_capacity as f64;
    metrics::gauge!("keplor_batch_queue_capacity").set(capacity);

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
                        // Sample post-dequeue: pairs with the enqueue
                        // sample in the pipeline so the gauge reflects
                        // both producers and the consumer.
                        let depth = rx.max_capacity() - rx.capacity();
                        metrics::gauge!("keplor_batch_queue_depth").set(depth as f64);
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
                        metrics::gauge!("keplor_batch_queue_depth").set(0.0);
                        tracing::info!("batch writer shut down cleanly");
                        return;
                    },
                }
            },
            _ = interval.tick() => {
                if !buffer.is_empty() {
                    flush(&store, &mut buffer).await;
                }
                // Tick samples too — keeps the gauge fresh during quiet
                // periods so dashboards don't see stale data.
                let depth = rx.max_capacity() - rx.capacity();
                metrics::gauge!("keplor_batch_queue_depth").set(depth as f64);
            },
        }
    }
}

/// Flush buffered writes via `spawn_blocking` so disk I/O never
/// blocks a tokio worker thread.
///
/// After the batch lands we call `wal_checkpoint` to rotate the in-WAL
/// events into segment files — KeplorDB queries only see rotated
/// segments, so without this step no event written through the
/// [`BatchWriter`] would ever become visible to `POST /v1/events`
/// follow-up reads. The cost is one segment file per flush cycle;
/// under the default 50 ms cadence that's ~1200 tiny segments/minute
/// at idle, which keplor's segment-level GC reclaims on the retention
/// schedule.
async fn flush(store: &Arc<KdbStore>, buffer: &mut Vec<WriteRequest>) {
    let pending = std::mem::take(buffer);
    let count = pending.len();

    let mut senders: Vec<Option<tokio::sync::oneshot::Sender<Result<EventId, StoreError>>>> =
        Vec::with_capacity(pending.len());
    let mut batch: Vec<LlmEvent> = Vec::with_capacity(pending.len());

    for req in pending {
        senders.push(req.result_tx);
        batch.push(req.event);
    }

    let store = Arc::clone(store);
    let result = tokio::task::spawn_blocking(move || {
        // append_batch_durable performs a single fsync per affected tier
        // at the end of the batch, eliminating the wal_sync_interval
        // data-loss window. Cost: ~10–50 µs extra per flush on NVMe.
        // At the default 50 ms cadence that's ~20 fsyncs/sec/tier — well
        // under what disks can sustain — and gives billing-grade durability
        // to *every* ingest path, not just X-Keplor-Durable batches.
        store.append_batch_durable(&batch)?;
        store.wal_checkpoint()?;
        Ok::<_, StoreError>(batch.iter().map(|e| e.id).collect::<Vec<_>>())
    })
    .await
    .unwrap_or_else(|e| Err(StoreError::Internal(e.to_string())));

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
                let _ = tx.send(Err(StoreError::Internal(msg.clone())));
            }
        },
    }
}
