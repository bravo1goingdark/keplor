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
        Self { batch_size: 64, flush_interval: Duration::from_millis(10), channel_capacity: 4096 }
    }
}

/// Async event writer that batches writes for throughput.
///
/// Callers send events via [`BatchWriter::write`]. A background task
/// accumulates them and flushes in bulk transactions, amortising
/// `BEGIN`/`COMMIT` and prepared-statement overhead across many events.
pub struct BatchWriter {
    tx: mpsc::Sender<WriteRequest>,
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
    /// The background flush task runs until all senders are dropped.
    pub fn new(store: Arc<Store>, config: BatchConfig) -> Self {
        let (tx, rx) = mpsc::channel(config.channel_capacity);
        tokio::spawn(flush_loop(store, rx, config));
        Self { tx }
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
                            flush(&store, &mut buffer);
                        }
                    },
                    None => {
                        if !buffer.is_empty() {
                            flush(&store, &mut buffer);
                        }
                        return;
                    },
                }
            },
            _ = interval.tick() => {
                if !buffer.is_empty() {
                    flush(&store, &mut buffer);
                }
            },
        }
    }
}

fn flush(store: &Store, buffer: &mut Vec<WriteRequest>) {
    let mut pending = std::mem::take(buffer);

    let batch: Vec<(LlmEvent, Bytes, Bytes)> = pending
        .iter()
        .map(|r| (r.event.clone(), r.req_body.clone(), r.resp_body.clone()))
        .collect();

    let result = store.append_batch(&batch);

    match result {
        Ok(ids) => {
            for (req, id) in pending.drain(..).zip(ids) {
                if let Some(tx) = req.result_tx {
                    let _ = tx.send(Ok(id));
                }
            }
        },
        Err(e) => {
            let msg = e.to_string();
            for req in pending.drain(..) {
                if let Some(tx) = req.result_tx {
                    let _ = tx.send(Err(StoreError::Compression(msg.clone())));
                }
            }
        },
    }
}
