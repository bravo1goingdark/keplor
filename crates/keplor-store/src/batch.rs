//! [`BatchWriter`] — sharded async event writer.
//!
//! Architecture:
//! - **N append loops**, one per shard, each owning its own bounded mpsc
//!   channel. Producers route events round-robin across shards. Each loop
//!   buffers up to `batch_size` and flushes via `append_batch_durable`.
//!   The keplordb engine internally round-robins each call onto its own
//!   WAL shard, so N concurrent flushes write N different WAL files with
//!   no fsync contention.
//! - **1 rotator loop** wakes every `flush_interval` and calls
//!   `wal_checkpoint` once to rotate WAL shards into segment files. This
//!   is what makes appended data visible to readers (queries only see
//!   segments, not WAL contents). Decoupling the rotator from the append
//!   path avoids an N² thundering herd of rotations.
//!
//! Read-visibility latency: ≤ `flush_interval` (unchanged vs the old
//! single-loop design). Write throughput: scales with `flush_shards`
//! up to the underlying engine's WAL-shard count.
//!
//! Per-event ordering across shards is not preserved. Within a single
//! `write_many` call, the input → output order in the returned `Vec` IS
//! preserved (each oneshot is awaited in input order), but the on-disk
//! commit order is non-deterministic across shards.
//!
//! On shutdown the rotator is aborted (via `Drop`); append loops exit
//! when their channels close, and each runs one final `wal_checkpoint`
//! to flush any drained buffer into a segment.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc;

use keplor_core::{EventId, LlmEvent};

use crate::error::StoreError;
use crate::kdb_store::KdbStore;

/// Configuration for the [`BatchWriter`].
#[derive(Debug, Clone)]
pub struct BatchConfig {
    /// Maximum events to accumulate before forcing a flush, per shard.
    pub batch_size: usize,
    /// Maximum time to wait before flushing a partial batch.
    pub flush_interval: Duration,
    /// Total bounded channel capacity across all shards. Each shard gets
    /// `channel_capacity / flush_shards` (rounded up). Set this to absorb
    /// expected burst traffic without 503-level back-pressure.
    pub channel_capacity: usize,
    /// Number of parallel append shards. Tunable via the
    /// `pipeline.flush_shards` config field; defaults to 4 to match the
    /// historical keplordb `wal_shard_count` default. Higher values let
    /// more cores work in parallel; values above the engine's
    /// `wal_shard_count` give diminishing returns.
    pub flush_shards: usize,
}

impl Default for BatchConfig {
    fn default() -> Self {
        Self {
            batch_size: 256,
            flush_interval: Duration::from_millis(50),
            channel_capacity: 8192,
            flush_shards: 4,
        }
    }
}

/// Async event writer that batches writes for throughput.
///
/// Callers send events via [`BatchWriter::write`]. Per-shard background
/// tasks accumulate them and flush in bulk transactions, amortising the
/// `BEGIN`/`COMMIT` and prepared-statement overhead across many events.
///
/// On shutdown, call [`BatchWriter::shutdown`] to drain pending events
/// before the process exits.
pub struct BatchWriter {
    /// Per-shard channel senders. Routed round-robin via `next_tx`.
    txs: Vec<mpsc::Sender<WriteRequest>>,
    next_tx: AtomicU64,
    /// Set to `true` after shutdown to reject new writes fast.
    shutdown: AtomicBool,
    total_capacity: usize,
    /// Append-loop join handles, one per shard. Mutex is uncontended on
    /// the hot path — only touched by `shutdown` and `Drop`.
    flush_handles: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    /// Single rotator-loop join handle. `Drop` aborts it.
    rotator_handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
}

struct WriteRequest {
    event: LlmEvent,
    result_tx: Option<tokio::sync::oneshot::Sender<Result<EventId, StoreError>>>,
}

impl BatchWriter {
    /// Spawn a new sharded batch writer backed by `store`.
    ///
    /// Spawns `config.flush_shards` append loops + 1 rotator. All run
    /// until [`BatchWriter::shutdown`] is called or the writer is dropped.
    pub fn new(store: Arc<KdbStore>, config: BatchConfig) -> Self {
        let shards = config.flush_shards.max(1);
        let per_shard_cap = config.channel_capacity.div_ceil(shards).max(1);
        // Per-shard batch threshold scales with shard count so the
        // fill-vs-interval tradeoff stays the same as the single-shard
        // baseline. Without this, each shard sees 1/N of the producers
        // and rarely fills `batch_size` — flushes fall through to the
        // interval tick and durable p50 collapses to `flush_interval`.
        // Floor at 8 to keep small batches efficient.
        let per_shard_batch_size = (config.batch_size / shards).max(8);

        let mut txs = Vec::with_capacity(shards);
        let mut handles = Vec::with_capacity(shards);
        for _ in 0..shards {
            let (tx, rx) = mpsc::channel(per_shard_cap);
            let shard_config = BatchConfig {
                batch_size: per_shard_batch_size,
                flush_interval: config.flush_interval,
                channel_capacity: per_shard_cap,
                flush_shards: 1,
            };
            let handle = tokio::spawn(append_loop(Arc::clone(&store), rx, shard_config));
            txs.push(tx);
            handles.push(handle);
        }

        let rotator = tokio::spawn(rotator_loop(Arc::clone(&store), config.flush_interval));

        let total_capacity = per_shard_cap * shards;
        metrics::gauge!("keplor_batch_queue_capacity").set(total_capacity as f64);

        Self {
            txs,
            next_tx: AtomicU64::new(0),
            shutdown: AtomicBool::new(false),
            total_capacity,
            flush_handles: Mutex::new(handles),
            rotator_handle: Mutex::new(Some(rotator)),
        }
    }

    /// Round-robin shard selector.
    fn pick_shard(&self) -> usize {
        let i = self.next_tx.fetch_add(1, Ordering::Relaxed) as usize;
        i % self.txs.len()
    }

    /// Submit an event for batched writing.
    ///
    /// Returns once the event is durably flushed to its shard's WAL.
    pub async fn write(&self, event: LlmEvent) -> Result<EventId, StoreError> {
        if self.shutdown.load(Ordering::Relaxed) {
            return Err(StoreError::ChannelClosed);
        }
        let (result_tx, result_rx) = tokio::sync::oneshot::channel();
        let shard = self.pick_shard();
        self.txs[shard]
            .send(WriteRequest { event, result_tx: Some(result_tx) })
            .await
            .map_err(|_| StoreError::ChannelClosed)?;
        result_rx.await.map_err(|_| StoreError::ChannelClosed)?
    }

    /// Submit multiple events and await all their flush confirmations.
    ///
    /// Events fan out across shards round-robin. Returned `Vec` preserves
    /// input order. Avoids the serial-await problem where each event
    /// would wait for a separate flush cycle.
    pub async fn write_many(&self, events: Vec<LlmEvent>) -> Vec<Result<EventId, StoreError>> {
        if self.shutdown.load(Ordering::Relaxed) {
            return events.iter().map(|_| Err(StoreError::ChannelClosed)).collect();
        }

        let mut receivers = Vec::with_capacity(events.len());
        for event in events {
            let (result_tx, result_rx) = tokio::sync::oneshot::channel();
            let shard = self.pick_shard();
            if self.txs[shard]
                .send(WriteRequest { event, result_tx: Some(result_tx) })
                .await
                .is_err()
            {
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
        let shard = self.pick_shard();
        self.txs[shard].try_send(WriteRequest { event, result_tx: None }).map_err(|e| match e {
            mpsc::error::TrySendError::Full(_) => StoreError::ChannelFull,
            mpsc::error::TrySendError::Closed(_) => StoreError::ChannelClosed,
        })
    }

    /// Total events currently queued across all shard channels.
    pub fn queue_depth(&self) -> usize {
        self.txs.iter().map(|tx| tx.max_capacity() - tx.capacity()).sum()
    }

    /// Aggregate maximum channel capacity across all shards.
    pub fn max_capacity(&self) -> usize {
        self.total_capacity
    }

    /// Gracefully shut down the batch writer.
    ///
    /// Sets the shutdown flag to reject new writes, then awaits each
    /// shard's append loop. Append loops only exit once the BatchWriter
    /// (and any cloned `Arc<BatchWriter>`) is dropped — until then
    /// senders stay alive and channels never close. Callers should call
    /// `shutdown()` first, then drop their `Arc<BatchWriter>`. The
    /// rotator is aborted via `Drop`.
    ///
    /// Returns `true` if every append loop completed within the deadline,
    /// `false` otherwise.
    pub async fn shutdown(&self, timeout: Duration) -> bool {
        self.shutdown.store(true, Ordering::Relaxed);

        let handles: Vec<_> = match self.flush_handles.lock() {
            Ok(mut g) => g.drain(..).collect(),
            Err(_) => return false,
        };
        if handles.is_empty() {
            return true;
        }

        let mut all_ok = true;
        let deadline = tokio::time::Instant::now() + timeout;
        for handle in handles {
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                handle.abort();
                all_ok = false;
                continue;
            }
            if tokio::time::timeout(remaining, handle).await.is_err() {
                all_ok = false;
            }
        }
        all_ok
    }
}

impl Drop for BatchWriter {
    fn drop(&mut self) {
        // Abort the rotator — it loops forever otherwise. Append loops
        // exit on their own when senders drop with this struct.
        if let Ok(mut g) = self.rotator_handle.lock() {
            if let Some(h) = g.take() {
                h.abort();
            }
        }
    }
}

async fn append_loop(
    store: Arc<KdbStore>,
    mut rx: mpsc::Receiver<WriteRequest>,
    config: BatchConfig,
) {
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
                        let depth = rx.max_capacity() - rx.capacity();
                        metrics::gauge!("keplor_batch_queue_depth").set(depth as f64);
                        if buffer.len() >= config.batch_size {
                            append_only(&store, &mut buffer).await;
                        }
                    },
                    None => {
                        // Channel closed — drain remaining events and run
                        // a final checkpoint so the drained data is
                        // visible to readers post-shutdown.
                        if !buffer.is_empty() {
                            tracing::info!(
                                events = buffer.len(),
                                "draining batch writer shard on shutdown"
                            );
                            append_only(&store, &mut buffer).await;
                        }
                        let store_for_ckpt = Arc::clone(&store);
                        let _ = tokio::task::spawn_blocking(move || {
                            store_for_ckpt.wal_checkpoint()
                        }).await;
                        metrics::gauge!("keplor_batch_queue_depth").set(0.0);
                        tracing::info!("batch writer shard shut down cleanly");
                        return;
                    },
                }
            },
            _ = interval.tick() => {
                if !buffer.is_empty() {
                    append_only(&store, &mut buffer).await;
                }
                let depth = rx.max_capacity() - rx.capacity();
                metrics::gauge!("keplor_batch_queue_depth").set(depth as f64);
            },
        }
    }
}

/// Periodic rotator: makes appended events visible to readers by
/// rotating WAL shards into segment files. One pass per `interval`,
/// shared across all append loops.
async fn rotator_loop(store: Arc<KdbStore>, interval: Duration) {
    let mut tick = tokio::time::interval(interval);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    // First tick fires immediately; skip it so we don't rotate before
    // any events have been appended.
    tick.tick().await;
    loop {
        tick.tick().await;
        let store = Arc::clone(&store);
        if let Err(e) = tokio::task::spawn_blocking(move || store.wal_checkpoint()).await {
            tracing::error!(error = %e, "rotator wal_checkpoint task panicked");
        }
    }
}

/// Append a buffered batch via `spawn_blocking` so disk I/O never
/// blocks a tokio worker thread. Does NOT rotate the WAL — that is
/// handled by [`rotator_loop`] on its own cadence.
async fn append_only(store: &Arc<KdbStore>, buffer: &mut Vec<WriteRequest>) {
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
        // at the end of the batch. The keplordb engine routes this call
        // round-robin onto a WAL shard, so N parallel append_only calls
        // from sibling shard loops fsync N different files in parallel.
        store.append_batch_durable(&batch)?;
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
