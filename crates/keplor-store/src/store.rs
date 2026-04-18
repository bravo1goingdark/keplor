//! [`Store`] — the local SQLite-backed storage engine.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use bytes::Bytes;
use rusqlite::{params, Connection, OptionalExtension};
use sha2::{Digest, Sha256};
use smol_str::SmolStr;

use keplor_core::{EventFlags, EventId, Latencies, LlmEvent, Provider, ProviderError, Usage};

use crate::components::{split_request, split_response, ComponentType};
use crate::compress::ZstdCoder;
use crate::error::StoreError;
use crate::filter::{Cursor, EventFilter};
use crate::migrations;

/// Statistics returned by [`Store::gc_expired`].
#[derive(Debug, Clone, Default)]
pub struct GcStats {
    /// Number of event rows deleted.
    pub events_deleted: usize,
    /// Number of blob rows deleted (refcount reached 0).
    pub blobs_deleted: usize,
}

/// Lightweight event projection for the HTTP API.
///
/// Contains only the fields needed by the query response — reads 16
/// columns instead of 42, avoiding allocation of hashes, error details,
/// trace metadata, and other fields the API doesn't expose.
#[derive(Debug, Clone)]
pub struct EventSummary {
    /// Primary key — time-sortable ULID.
    pub id: keplor_core::EventId,
    /// Wall-clock capture time in nanoseconds.
    pub ts_ns: i64,
    /// Caller-provided user id.
    pub user_id: Option<String>,
    /// API key id.
    pub api_key_id: Option<String>,
    /// Provider id key (e.g. `"openai"`).
    pub provider: String,
    /// Model name.
    pub model: String,
    /// Request endpoint.
    pub endpoint: String,
    /// HTTP status code.
    pub http_status: Option<u16>,
    /// Input tokens.
    pub input_tokens: u32,
    /// Output tokens.
    pub output_tokens: u32,
    /// Cache-read input tokens.
    pub cache_read_input_tokens: u32,
    /// Cache-creation input tokens.
    pub cache_creation_input_tokens: u32,
    /// Reasoning tokens.
    pub reasoning_tokens: u32,
    /// Cost in nanodollars.
    pub cost_nanodollars: i64,
    /// Time to first token (ms).
    pub ttft_ms: Option<u32>,
    /// Total latency (ms).
    pub total_ms: u32,
    /// Whether the request was streaming.
    pub streaming: bool,
    /// Ingestion source.
    pub source: Option<String>,
    /// Error type (e.g. `"rate_limited"`, `"upstream_429"`).
    pub error_type: Option<String>,
    /// Arbitrary metadata as JSON text.
    pub metadata_json: Option<String>,
}

/// Cost + event count from a quota query.
#[derive(Debug, Clone)]
pub struct QuotaSummary {
    /// Total cost in nanodollars.
    pub cost_nanodollars: i64,
    /// Number of events matching the filter.
    pub event_count: i64,
}

/// A single row from the `daily_rollups` table.
#[derive(Debug, Clone)]
pub struct RollupRow {
    /// Day boundary as epoch seconds.
    pub day: i64,
    /// User id (empty string if not set).
    pub user_id: String,
    /// API key id (empty string if not set).
    pub api_key_id: String,
    /// Provider id key.
    pub provider: String,
    /// Model name.
    pub model: String,
    /// Number of events.
    pub event_count: i64,
    /// Number of events with http_status >= 400.
    pub error_count: i64,
    /// Total input tokens.
    pub input_tokens: i64,
    /// Total output tokens.
    pub output_tokens: i64,
    /// Total cache-read input tokens.
    pub cache_read_input_tokens: i64,
    /// Total cache-creation input tokens.
    pub cache_creation_input_tokens: i64,
    /// Total cost in nanodollars.
    pub cost_nanodollars: i64,
}

/// An aggregated stats row (optionally grouped by model).
#[derive(Debug, Clone)]
pub struct AggregateRow {
    /// Provider (empty string when not grouped).
    pub provider: String,
    /// Model name (empty string when not grouped).
    pub model: String,
    /// Number of events.
    pub event_count: i64,
    /// Number of error events.
    pub error_count: i64,
    /// Total input tokens.
    pub input_tokens: i64,
    /// Total output tokens.
    pub output_tokens: i64,
    /// Total cache-read input tokens.
    pub cache_read_input_tokens: i64,
    /// Total cache-creation input tokens.
    pub cache_creation_input_tokens: i64,
    /// Total cost in nanodollars.
    pub cost_nanodollars: i64,
}

/// Default number of read connections in the pool.
const DEFAULT_READ_POOL_SIZE: usize = 4;

/// Internal trait for opening additional connections (file vs. in-memory).
trait ConnOpener {
    fn open(&self) -> Result<Connection, StoreError>;
}

struct FileOpener(PathBuf);

impl ConnOpener for FileOpener {
    fn open(&self) -> Result<Connection, StoreError> {
        Connection::open(&self.0).map_err(StoreError::from)
    }
}

/// In-memory opener using a unique shared-cache name so that parallel
/// tests each get their own database while connections within one Store
/// still share state.
struct MemoryOpener(String);

impl ConnOpener for MemoryOpener {
    fn open(&self) -> Result<Connection, StoreError> {
        Connection::open(&self.0).map_err(StoreError::from)
    }
}

/// Local SQLite store for LLM events and their payload blobs.
///
/// Uses a **read/write split** connection pool:
/// - One dedicated write connection (`write_conn`) for all mutations
/// - N read connections (`read_pool`) for concurrent queries
///
/// SQLite WAL mode (set in pragmas) allows readers to proceed without
/// blocking writers and vice-versa, as long as they use separate
/// connections.
pub struct Store {
    write_conn: Mutex<Connection>,
    read_pool: Vec<Mutex<Connection>>,
    read_idx: AtomicUsize,
    coder: ZstdCoder,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store")
            .field("read_pool_size", &self.read_pool.len())
            .finish_non_exhaustive()
    }
}

impl Store {
    /// Open (or create) a store at the given path.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        Self::open_with_pool_size(path, DEFAULT_READ_POOL_SIZE)
    }

    /// Open a store with a custom read pool size.
    pub fn open_with_pool_size(path: &Path, read_pool_size: usize) -> Result<Self, StoreError> {
        let pool_size = read_pool_size.max(1);
        let write_conn = Connection::open(path)?;
        let opener = FileOpener(path.to_path_buf());
        Self::init(write_conn, &opener, pool_size)
    }

    /// Open an in-memory store (for tests).
    ///
    /// Uses a unique SQLite shared-cache URI so that the write and read
    /// connections all see the same in-memory database, while parallel
    /// test instances each get their own.
    pub fn open_in_memory() -> Result<Self, StoreError> {
        static COUNTER: AtomicUsize = AtomicUsize::new(0);
        let id = COUNTER.fetch_add(1, Ordering::Relaxed);
        let uri = format!("file:keplor_mem_{id}?mode=memory&cache=shared");
        let write_conn = Connection::open(&uri)?;
        let opener = MemoryOpener(uri);
        Self::init(write_conn, &opener, DEFAULT_READ_POOL_SIZE)
    }

    fn init(
        write_conn: Connection,
        opener: &dyn ConnOpener,
        pool_size: usize,
    ) -> Result<Self, StoreError> {
        migrations::apply_pragmas(&write_conn)?;
        migrations::migrate(&write_conn)?;
        let mut coder = ZstdCoder::new();
        Self::load_dicts(&write_conn, &mut coder)?;

        let mut read_pool = Vec::with_capacity(pool_size);
        for _ in 0..pool_size {
            let rc = opener.open()?;
            migrations::apply_pragmas(&rc)?;
            read_pool.push(Mutex::new(rc));
        }

        Ok(Self {
            write_conn: Mutex::new(write_conn),
            read_pool,
            read_idx: AtomicUsize::new(0),
            coder,
        })
    }

    /// Acquire a read connection from the pool (round-robin).
    fn read_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, StoreError> {
        let idx = self.read_idx.fetch_add(1, Ordering::Relaxed) % self.read_pool.len();
        self.read_pool[idx].lock().map_err(|e| StoreError::Compression(e.to_string()))
    }

    /// Acquire the write connection.
    fn write_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, StoreError> {
        self.write_conn.lock().map_err(|e| StoreError::Compression(e.to_string()))
    }

    /// Append an event with its raw request/response bodies.
    ///
    /// Bodies are split into components, SHA-256 hashed, compressed, and
    /// stored with refcount-based deduplication.  Compression is skipped
    /// entirely on dedup hits.
    pub fn append_event(
        &self,
        event: &LlmEvent,
        req_body: &Bytes,
        resp_body: &Bytes,
    ) -> Result<EventId, StoreError> {
        let request_sha = sha256_bytes(req_body);
        let response_sha = sha256_bytes(resp_body);

        let req_components = split_request(&event.provider, req_body);
        let resp_components = split_response(resp_body);

        let provider_key = event.provider.id_key();

        let conn = self.write_conn()?;
        let tx = conn.unchecked_transaction()?;

        let mut request_blob_id: Option<[u8; 32]> = None;
        let mut response_blob_id: Option<[u8; 32]> = None;
        let mut component_links: Vec<(&str, [u8; 32])> =
            Vec::with_capacity(req_components.len() + resp_components.len());

        for comp in &req_components {
            let sha = sha256_bytes(&comp.data);
            self.upsert_blob(&tx, &sha, &comp.data, comp.kind.as_str(), provider_key)?;
            component_links.push((comp.kind.as_str(), sha));
            if comp.kind == ComponentType::Messages || comp.kind == ComponentType::Raw {
                request_blob_id = Some(sha);
            }
        }

        for comp in &resp_components {
            let sha = sha256_bytes(&comp.data);
            self.upsert_blob(&tx, &sha, &comp.data, comp.kind.as_str(), provider_key)?;
            component_links.push((comp.kind.as_str(), sha));
            response_blob_id = Some(sha);
        }

        let error_type = event.error.as_ref().map(error_type_str);
        let error_message = event.error.as_ref().map(|e| e.to_string());

        let metadata_str =
            event.metadata.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default());

        tx.execute(
            "INSERT INTO llm_events(
                id, ts_ns, user_id, api_key_id, org_id, project_id, route_id,
                provider, model, model_family, endpoint, method, http_status,
                input_tokens, output_tokens, cache_read_input_tokens,
                cache_creation_input_tokens, reasoning_tokens,
                audio_input_tokens, audio_output_tokens, image_tokens, tool_use_tokens,
                cost_nanodollars, latency_ttft_ms, latency_total_ms, time_to_close_ms,
                streaming, tool_calls, reasoning, stream_incomplete,
                error_type, error_message,
                request_sha256, response_sha256, request_blob_id, response_blob_id,
                client_ip, user_agent, request_id, trace_id,
                source, ingested_at, metadata_json
             ) VALUES(
                ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                ?8, ?9, ?10, ?11, ?12, ?13,
                ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22,
                ?23, ?24, ?25, ?26,
                ?27, ?28, ?29, ?30,
                ?31, ?32,
                ?33, ?34, ?35, ?36,
                ?37, ?38, ?39, ?40,
                ?41, ?42, ?43
             )",
            params![
                event.id.as_ulid().to_bytes().as_slice(),
                event.ts_ns,
                event.user_id.as_ref().map(|u| u.as_str()),
                event.api_key_id.as_ref().map(|a| a.as_str()),
                event.org_id.as_ref().map(|o| o.as_str()),
                event.project_id.as_ref().map(|p| p.as_str()),
                event.route_id.as_str(),
                provider_key,
                event.model.as_str(),
                event.model_family.as_deref(),
                event.endpoint.as_str(),
                event.method.as_str(),
                event.http_status.map(i64::from),
                i64::from(event.usage.input_tokens),
                i64::from(event.usage.output_tokens),
                i64::from(event.usage.cache_read_input_tokens),
                i64::from(event.usage.cache_creation_input_tokens),
                i64::from(event.usage.reasoning_tokens),
                i64::from(event.usage.audio_input_tokens),
                i64::from(event.usage.audio_output_tokens),
                i64::from(event.usage.image_tokens),
                i64::from(event.usage.tool_use_tokens),
                event.cost_nanodollars,
                event.latency.ttft_ms.map(i64::from),
                i64::from(event.latency.total_ms),
                event.latency.time_to_close_ms.map(i64::from),
                event.flags.contains(EventFlags::STREAMING) as i64,
                event.flags.contains(EventFlags::TOOL_CALLS) as i64,
                event.flags.contains(EventFlags::REASONING) as i64,
                event.flags.contains(EventFlags::STREAM_INCOMPLETE) as i64,
                error_type,
                error_message,
                &request_sha[..],
                &response_sha[..],
                request_blob_id.as_ref().map(|b| &b[..]),
                response_blob_id.as_ref().map(|b| &b[..]),
                event.client_ip.map(|ip| ip.to_string()),
                event.user_agent.as_deref(),
                event.request_id.as_deref(),
                event.trace_id.map(|t| t.to_string()),
                event.source.as_deref(),
                event.ingested_at,
                metadata_str.as_deref(),
            ],
        )?;

        let event_id_bytes = event.id.as_ulid().to_bytes();
        for (comp_type, blob_sha) in &component_links {
            tx.execute(
                "INSERT OR IGNORE INTO event_components(event_id, component_type, blob_sha256)
                 VALUES(?1, ?2, ?3)",
                params![&event_id_bytes[..], comp_type, &blob_sha[..]],
            )?;
        }

        tx.commit()?;
        Ok(event.id)
    }

    /// Append a batch of events in a single transaction.
    ///
    /// All SHA-256 hashing and zstd compression runs **before** the mutex
    /// is acquired, so the lock is held only for the SQLite writes.
    /// Compression is skipped for blobs already seen within the batch.
    /// Prepared statements are reused across all events.
    pub fn append_batch(
        &self,
        events: &[(LlmEvent, Bytes, Bytes)],
    ) -> Result<Vec<EventId>, StoreError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }

        // ── Pre-lock: hash + split + compress (dedup within batch) ────
        let mut seen_shas: std::collections::HashSet<[u8; 32]> =
            std::collections::HashSet::with_capacity(events.len() * 3);
        let mut unique_blobs: Vec<PreparedBlob> = Vec::with_capacity(events.len() * 3);

        struct BatchEvent<'a> {
            event: &'a LlmEvent,
            event_id_bytes: [u8; 16],
            provider_key: &'a str,
            request_sha: [u8; 32],
            response_sha: [u8; 32],
            request_blob_id: Option<[u8; 32]>,
            response_blob_id: Option<[u8; 32]>,
            component_links: Vec<(&'static str, [u8; 32])>,
            error_type: Option<&'static str>,
            error_message: Option<String>,
            // Pre-formatted outside the lock to avoid allocations under Mutex.
            client_ip_str: Option<String>,
            trace_id_str: Option<String>,
            metadata_str: Option<String>,
        }

        let mut batch_events: Vec<BatchEvent<'_>> = Vec::with_capacity(events.len());

        for (event, req_body, resp_body) in events {
            // Reuse pre-computed SHAs from LlmEvent when available (non-zero),
            // avoiding redundant SHA-256 computations on the hot path.
            let request_sha = if event.request_sha256 != [0u8; 32] {
                event.request_sha256
            } else {
                sha256_bytes(req_body)
            };
            let response_sha = if event.response_sha256 != [0u8; 32] {
                event.response_sha256
            } else {
                sha256_bytes(resp_body)
            };
            let provider_key = event.provider.id_key();

            let req_components = split_request(&event.provider, req_body);
            let resp_components = split_response(resp_body);

            let total = req_components.len() + resp_components.len();
            let mut component_links = Vec::with_capacity(total);
            let mut request_blob_id: Option<[u8; 32]> = None;
            let mut response_blob_id: Option<[u8; 32]> = None;

            for comp in req_components.iter().chain(resp_components.iter()) {
                let sha = sha256_bytes(&comp.data);
                component_links.push((comp.kind.as_str(), sha));

                if comp.kind == ComponentType::Messages || comp.kind == ComponentType::Raw {
                    request_blob_id = Some(sha);
                }
                if comp.kind == ComponentType::Response {
                    response_blob_id = Some(sha);
                }

                if seen_shas.insert(sha) {
                    let dict_key = crate::compress::DictKey {
                        provider: provider_key.into(),
                        component_type: comp.kind.as_str().into(),
                    };
                    let has_dict = self.coder.has_dict(&dict_key);
                    let compressed = self.coder.compress(&comp.data, Some(&dict_key))?;
                    unique_blobs.push(PreparedBlob {
                        sha,
                        component_type: comp.kind.as_str(),
                        provider_key,
                        uncompressed_len: comp.data.len(),
                        compressed,
                        used_dict: has_dict,
                    });
                }
            }

            batch_events.push(BatchEvent {
                event,
                event_id_bytes: event.id.as_ulid().to_bytes(),
                provider_key,
                request_sha,
                response_sha,
                request_blob_id,
                response_blob_id,
                component_links,
                error_type: event.error.as_ref().map(error_type_str),
                error_message: event.error.as_ref().map(|e| e.to_string()),
                client_ip_str: event.client_ip.map(|ip| ip.to_string()),
                trace_id_str: event.trace_id.map(|t| t.to_string()),
                metadata_str: event.metadata.as_ref().and_then(|m| serde_json::to_string(m).ok()),
            });
        }

        // ── Lock + single transaction ─────────────────────────────────
        let conn = self.write_conn()?;
        let tx = conn.unchecked_transaction()?;

        // Insert unique blobs with ON CONFLICT for cross-batch dedup.
        {
            let mut blob_stmt = tx.prepare_cached(
                "INSERT INTO payload_blobs(
                    sha256, component_type, provider, compression, dict_id,
                    uncompressed_size, compressed_size, refcount, hit_count,
                    data, first_seen_at
                 ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, 0, ?8, strftime('%s','now'))
                 ON CONFLICT(sha256) DO UPDATE SET
                    refcount = refcount + 1,
                    hit_count = hit_count + 1",
            )?;

            // First pass: insert each unique blob once.
            for blob in &unique_blobs {
                let (compression, dict_id): (&str, Option<String>) = if blob.used_dict {
                    ("zstd_dict", Some(format!("{}_{}", blob.provider_key, blob.component_type)))
                } else {
                    ("zstd_raw", None)
                };
                blob_stmt.execute(params![
                    &blob.sha[..],
                    blob.component_type,
                    blob.provider_key,
                    compression,
                    dict_id,
                    blob.uncompressed_len as i64,
                    blob.compressed.len() as i64,
                    &blob.compressed[..],
                ])?;
            }

            // Second pass: bump refcount for intra-batch duplicates.
            // One UPDATE per duplicate SHA (instead of N-1 individual UPDATEs).
            let mut bump_stmt = tx.prepare_cached(
                "UPDATE payload_blobs SET refcount = refcount + ?1, hit_count = hit_count + ?1
                 WHERE sha256 = ?2",
            )?;
            let mut ref_counts: std::collections::HashMap<[u8; 32], usize> =
                std::collections::HashMap::with_capacity(unique_blobs.len());
            for be in &batch_events {
                for &(_, sha) in &be.component_links {
                    *ref_counts.entry(sha).or_insert(0) += 1;
                }
            }
            for (sha, count) in &ref_counts {
                if *count > 1 {
                    bump_stmt.execute(params![(*count - 1) as i64, &sha[..]])?;
                }
            }
        }

        {
            let mut event_stmt = tx.prepare_cached(
                "INSERT INTO llm_events(
                    id, ts_ns, user_id, api_key_id, org_id, project_id, route_id,
                    provider, model, model_family, endpoint, method, http_status,
                    input_tokens, output_tokens, cache_read_input_tokens,
                    cache_creation_input_tokens, reasoning_tokens,
                    audio_input_tokens, audio_output_tokens, image_tokens, tool_use_tokens,
                    cost_nanodollars, latency_ttft_ms, latency_total_ms, time_to_close_ms,
                    streaming, tool_calls, reasoning, stream_incomplete,
                    error_type, error_message,
                    request_sha256, response_sha256, request_blob_id, response_blob_id,
                    client_ip, user_agent, request_id, trace_id,
                    source, ingested_at, metadata_json
                 ) VALUES(
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                    ?8, ?9, ?10, ?11, ?12, ?13,
                    ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22,
                    ?23, ?24, ?25, ?26,
                    ?27, ?28, ?29, ?30,
                    ?31, ?32,
                    ?33, ?34, ?35, ?36,
                    ?37, ?38, ?39, ?40,
                    ?41, ?42, ?43
                 )",
            )?;

            for pe in &batch_events {
                let e = pe.event;
                event_stmt.execute(params![
                    pe.event_id_bytes.as_slice(),
                    e.ts_ns,
                    e.user_id.as_ref().map(|u| u.as_str()),
                    e.api_key_id.as_ref().map(|a| a.as_str()),
                    e.org_id.as_ref().map(|o| o.as_str()),
                    e.project_id.as_ref().map(|p| p.as_str()),
                    e.route_id.as_str(),
                    pe.provider_key,
                    e.model.as_str(),
                    e.model_family.as_deref(),
                    e.endpoint.as_str(),
                    e.method.as_str(),
                    e.http_status.map(i64::from),
                    i64::from(e.usage.input_tokens),
                    i64::from(e.usage.output_tokens),
                    i64::from(e.usage.cache_read_input_tokens),
                    i64::from(e.usage.cache_creation_input_tokens),
                    i64::from(e.usage.reasoning_tokens),
                    i64::from(e.usage.audio_input_tokens),
                    i64::from(e.usage.audio_output_tokens),
                    i64::from(e.usage.image_tokens),
                    i64::from(e.usage.tool_use_tokens),
                    e.cost_nanodollars,
                    e.latency.ttft_ms.map(i64::from),
                    i64::from(e.latency.total_ms),
                    e.latency.time_to_close_ms.map(i64::from),
                    e.flags.contains(EventFlags::STREAMING) as i64,
                    e.flags.contains(EventFlags::TOOL_CALLS) as i64,
                    e.flags.contains(EventFlags::REASONING) as i64,
                    e.flags.contains(EventFlags::STREAM_INCOMPLETE) as i64,
                    pe.error_type,
                    &pe.error_message,
                    &pe.request_sha[..],
                    &pe.response_sha[..],
                    pe.request_blob_id.as_ref().map(|b| &b[..]),
                    pe.response_blob_id.as_ref().map(|b| &b[..]),
                    &pe.client_ip_str,
                    e.user_agent.as_deref(),
                    e.request_id.as_deref(),
                    &pe.trace_id_str,
                    e.source.as_deref(),
                    e.ingested_at,
                    pe.metadata_str.as_deref(),
                ])?;
            }
        }

        {
            let mut link_stmt = tx.prepare_cached(
                "INSERT OR IGNORE INTO event_components(event_id, component_type, blob_sha256)
                 VALUES(?1, ?2, ?3)",
            )?;

            for be in &batch_events {
                for (comp_type, blob_sha) in &be.component_links {
                    link_stmt.execute(params![&be.event_id_bytes[..], comp_type, &blob_sha[..]])?;
                }
            }
        }

        tx.commit()?;
        Ok(batch_events.iter().map(|be| be.event.id).collect())
    }

    /// Single-trip INSERT ON CONFLICT: compress + insert, dedup on conflict.
    fn upsert_blob(
        &self,
        conn: &Connection,
        sha: &[u8; 32],
        data: &[u8],
        component_type: &str,
        provider: &str,
    ) -> Result<(), StoreError> {
        let dict_key = crate::compress::DictKey {
            provider: provider.into(),
            component_type: component_type.into(),
        };
        let has_dict = self.coder.has_dict(&dict_key);
        let compressed = self.coder.compress(data, Some(&dict_key))?;
        let (compression, dict_id): (&str, Option<String>) = if has_dict {
            ("zstd_dict", Some(format!("{provider}_{component_type}")))
        } else {
            ("zstd_raw", None)
        };
        conn.execute(
            "INSERT INTO payload_blobs(
                sha256, component_type, provider, compression, dict_id,
                uncompressed_size, compressed_size, refcount, hit_count,
                data, first_seen_at
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, 1, 0, ?8, strftime('%s','now'))
             ON CONFLICT(sha256) DO UPDATE SET
                refcount = refcount + 1,
                hit_count = hit_count + 1",
            params![
                &sha[..],
                component_type,
                provider,
                compression,
                dict_id,
                data.len() as i64,
                compressed.len() as i64,
                &compressed[..],
            ],
        )?;
        Ok(())
    }

    /// Retrieve an event by id.
    pub fn get_event(&self, id: &EventId) -> Result<Option<LlmEvent>, StoreError> {
        let conn = self.read_conn()?;
        let id_bytes = id.as_ulid().to_bytes();

        conn.query_row("SELECT * FROM llm_events WHERE id = ?1", [&id_bytes[..]], row_to_event)
            .optional()
            .map_err(StoreError::from)
    }

    /// Retrieve blob data for a specific component of an event.
    pub fn get_component(
        &self,
        event_id: &EventId,
        component_type: &str,
    ) -> Result<Option<Bytes>, StoreError> {
        let conn = self.read_conn()?;
        let id_bytes = event_id.as_ulid().to_bytes();

        let row: Option<(Vec<u8>, String, String)> = conn
            .query_row(
                "SELECT pb.data, pb.provider, pb.compression FROM event_components ec
                 JOIN payload_blobs pb ON pb.sha256 = ec.blob_sha256
                 WHERE ec.event_id = ?1 AND ec.component_type = ?2",
                params![&id_bytes[..], component_type],
                |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
            )
            .optional()?;

        let Some((compressed, provider, compression)) = row else {
            return Ok(None);
        };

        // Only use dict for blobs that were compressed with one.
        // Old blobs (compression = 'zstd_raw') decompress without dict.
        let dict_key = if compression == "zstd_dict" {
            Some(crate::compress::DictKey {
                provider: provider.into(),
                component_type: component_type.into(),
            })
        } else {
            None
        };
        let decompressed = self.coder.decompress(&compressed, dict_key.as_ref())?;
        Ok(Some(Bytes::from(decompressed)))
    }

    /// Query events with filters and cursor-based pagination.
    pub fn query(
        &self,
        filter: &EventFilter,
        limit: u32,
        cursor: Option<Cursor>,
    ) -> Result<Vec<LlmEvent>, StoreError> {
        let conn = self.read_conn()?;

        let mut sql = String::with_capacity(256);
        sql.push_str("SELECT * FROM llm_events WHERE 1=1");

        // Fixed-size storage for bind values avoids heap allocation per
        // filter predicate.  Maximum 8 predicates (7 filter fields + cursor).
        let mut bind_storage: [Option<Box<dyn rusqlite::types::ToSql>>; 8] = Default::default();
        let mut idx = 0usize;

        macro_rules! add_filter {
            ($cond:expr, $val:expr) => {
                if let Some(ref v) = $val {
                    idx += 1;
                    sql.push_str(&format!(concat!(" AND ", $cond, " = ?{}"), idx));
                    bind_storage[idx - 1] = Some(Box::new(v.to_string()));
                }
            };
        }

        add_filter!("user_id", filter.user_id);
        add_filter!("api_key_id", filter.api_key_id);
        add_filter!("model", filter.model);
        add_filter!("provider", filter.provider);
        add_filter!("source", filter.source);

        if let Some(from) = filter.from_ts_ns {
            idx += 1;
            sql.push_str(&format!(" AND ts_ns >= ?{idx}"));
            bind_storage[idx - 1] = Some(Box::new(from));
        }
        if let Some(to) = filter.to_ts_ns {
            idx += 1;
            sql.push_str(&format!(" AND ts_ns <= ?{idx}"));
            bind_storage[idx - 1] = Some(Box::new(to));
        }
        if let Some(c) = cursor {
            idx += 1;
            sql.push_str(&format!(" AND ts_ns < ?{idx}"));
            bind_storage[idx - 1] = Some(Box::new(c.0));
        }

        sql.push_str(" ORDER BY ts_ns DESC LIMIT ");
        sql.push_str(&limit.to_string());

        let mut stmt = conn.prepare_cached(&sql)?;
        let params_ref: Vec<&dyn rusqlite::types::ToSql> = bind_storage[..idx]
            .iter()
            .map(|slot| slot.as_ref().map(|b| b.as_ref()).unwrap_or(&rusqlite::types::Null))
            .collect();

        let mut events = Vec::with_capacity(limit.min(256) as usize);
        let rows = stmt.query_map(params_ref.as_slice(), row_to_event)?;
        for row in rows {
            events.push(row?);
        }
        Ok(events)
    }

    /// Narrow query returning only the fields needed by the HTTP API.
    ///
    /// Reads 19 columns instead of 43, avoiding allocation of unused
    /// fields (hashes, trace metadata, etc.).
    pub fn query_summary(
        &self,
        filter: &EventFilter,
        limit: u32,
        cursor: Option<Cursor>,
    ) -> Result<Vec<EventSummary>, StoreError> {
        let conn = self.read_conn()?;

        let mut sql = String::with_capacity(256);
        sql.push_str(
            "SELECT id, ts_ns, user_id, api_key_id, provider, model, endpoint, http_status, \
             input_tokens, output_tokens, cache_read_input_tokens, cache_creation_input_tokens, reasoning_tokens, \
             cost_nanodollars, latency_ttft_ms, latency_total_ms, streaming, source, \
             error_type, metadata_json \
             FROM llm_events WHERE 1=1",
        );

        let mut bind_storage: [Option<Box<dyn rusqlite::types::ToSql>>; 12] = Default::default();
        let mut idx = 0usize;

        macro_rules! add_filter {
            ($cond:expr, $val:expr) => {
                if let Some(ref v) = $val {
                    idx += 1;
                    sql.push_str(&format!(concat!(" AND ", $cond, " = ?{}"), idx));
                    bind_storage[idx - 1] = Some(Box::new(v.to_string()));
                }
            };
        }

        add_filter!("user_id", filter.user_id);
        add_filter!("api_key_id", filter.api_key_id);
        add_filter!("model", filter.model);
        add_filter!("provider", filter.provider);
        add_filter!("source", filter.source);

        if let Some(from) = filter.from_ts_ns {
            idx += 1;
            sql.push_str(&format!(" AND ts_ns >= ?{idx}"));
            bind_storage[idx - 1] = Some(Box::new(from));
        }
        if let Some(to) = filter.to_ts_ns {
            idx += 1;
            sql.push_str(&format!(" AND ts_ns <= ?{idx}"));
            bind_storage[idx - 1] = Some(Box::new(to));
        }
        if let Some(min) = filter.http_status_min {
            idx += 1;
            sql.push_str(&format!(" AND http_status >= ?{idx}"));
            bind_storage[idx - 1] = Some(Box::new(i64::from(min)));
        }
        if let Some(max) = filter.http_status_max {
            idx += 1;
            sql.push_str(&format!(" AND http_status < ?{idx}"));
            bind_storage[idx - 1] = Some(Box::new(i64::from(max)));
        }
        if let Some(ref tag) = filter.meta_user_tag {
            idx += 1;
            sql.push_str(&format!(" AND json_extract(metadata_json, '$.user_tag') = ?{idx}"));
            bind_storage[idx - 1] = Some(Box::new(tag.to_string()));
        }
        if let Some(ref tag) = filter.meta_session_tag {
            idx += 1;
            sql.push_str(&format!(" AND json_extract(metadata_json, '$.session_tag') = ?{idx}"));
            bind_storage[idx - 1] = Some(Box::new(tag.to_string()));
        }
        if let Some(c) = cursor {
            idx += 1;
            sql.push_str(&format!(" AND ts_ns < ?{idx}"));
            bind_storage[idx - 1] = Some(Box::new(c.0));
        }

        sql.push_str(" ORDER BY ts_ns DESC LIMIT ");
        sql.push_str(&limit.to_string());

        let mut stmt = conn.prepare_cached(&sql)?;
        let params_ref: Vec<&dyn rusqlite::types::ToSql> = bind_storage[..idx]
            .iter()
            .map(|slot| slot.as_ref().map(|b| b.as_ref()).unwrap_or(&rusqlite::types::Null))
            .collect();

        let mut events = Vec::with_capacity(limit.min(256) as usize);
        let rows = stmt.query_map(params_ref.as_slice(), row_to_summary)?;
        for row in rows {
            events.push(row?);
        }
        Ok(events)
    }

    /// Collect raw (uncompressed) payload samples for dictionary training.
    ///
    /// Returns up to `max_samples` decompressed blobs of the given
    /// `component_type` and `provider`.
    pub fn collect_dict_samples(
        &self,
        provider: &str,
        component_type: &str,
        max_samples: usize,
    ) -> Result<Vec<Vec<u8>>, StoreError> {
        let conn = self.read_conn()?;

        let mut stmt = conn.prepare_cached(
            "SELECT data FROM payload_blobs
             WHERE provider = ?1 AND component_type = ?2
             ORDER BY hit_count DESC
             LIMIT ?3",
        )?;

        let rows = stmt.query_map(params![provider, component_type, max_samples as i64], |r| {
            r.get::<_, Vec<u8>>(0)
        })?;

        let mut samples = Vec::with_capacity(max_samples);
        for row in rows {
            let compressed = row?;
            let decompressed = self.coder.decompress(&compressed, None)?;
            samples.push(decompressed);
        }
        Ok(samples)
    }

    /// Train a zstd dictionary from stored samples and persist it.
    ///
    /// Reads the most-referenced blobs of the given `(provider,
    /// component_type)`, trains a dictionary, stores it in `zstd_dicts`,
    /// and registers it in the coder for future compression.
    ///
    /// Returns the dict id on success, or `None` if too few samples.
    pub fn train_dict(
        &mut self,
        provider: &str,
        component_type: &str,
        max_samples: usize,
        dict_size: usize,
    ) -> Result<Option<String>, StoreError> {
        let samples = self.collect_dict_samples(provider, component_type, max_samples)?;
        if samples.len() < 10 {
            return Ok(None);
        }

        let sample_refs: Vec<&[u8]> = samples.iter().map(|s| s.as_slice()).collect();
        let dict_bytes = zstd::dict::from_samples(&sample_refs, dict_size)
            .map_err(|e| StoreError::Compression(format!("dict training failed: {e}")))?;

        let dict_id = format!("{provider}_{component_type}");

        {
            let conn = self.write_conn()?;
            conn.execute(
                "INSERT OR REPLACE INTO zstd_dicts(id, provider, component_type, sample_count, created_at, data)
                 VALUES(?1, ?2, ?3, ?4, strftime('%s','now'), ?5)",
                params![&dict_id, provider, component_type, samples.len() as i64, &dict_bytes],
            )?;
        }

        let key = crate::compress::DictKey {
            provider: provider.into(),
            component_type: component_type.into(),
        };
        self.coder.register_dict(key, dict_bytes)?;

        Ok(Some(dict_id))
    }

    /// Load all trained dictionaries from the `zstd_dicts` table.
    ///
    /// Called during [`Store::init`] to pre-populate the coder.
    fn load_dicts(conn: &Connection, coder: &mut ZstdCoder) -> Result<(), StoreError> {
        let mut stmt = conn.prepare("SELECT provider, component_type, data FROM zstd_dicts")?;
        let rows = stmt.query_map([], |r| {
            Ok((r.get::<_, String>(0)?, r.get::<_, String>(1)?, r.get::<_, Vec<u8>>(2)?))
        })?;
        for row in rows {
            let (provider, component_type, data) = row?;
            let key = crate::compress::DictKey {
                provider: provider.into(),
                component_type: component_type.into(),
            };
            coder.register_dict(key, data)?;
        }
        Ok(())
    }

    /// Roll up a day's events into `daily_rollups`.
    ///
    /// Uses DELETE+INSERT (not INSERT OR REPLACE) so re-running is always
    /// clean — handles edge cases where group-by dimensions change after
    /// event corrections.
    pub fn rollup_day(&self, day_epoch: i64) -> Result<(), StoreError> {
        let conn = self.write_conn()?;
        let next_day = day_epoch + 86400;
        let from_ns = day_epoch * 1_000_000_000i64;
        let to_ns = next_day * 1_000_000_000i64;

        let tx = conn.unchecked_transaction()?;

        tx.execute("DELETE FROM daily_rollups WHERE day = ?1", [day_epoch])?;

        tx.execute(
            "INSERT INTO daily_rollups(day, user_id, api_key_id, provider, model,
                event_count, error_count, input_tokens, output_tokens,
                cache_read_input_tokens, cache_creation_input_tokens, cost_nanodollars)
             SELECT ?1,
                    COALESCE(user_id, ''),
                    COALESCE(api_key_id, ''),
                    COALESCE(provider, ''),
                    COALESCE(model, ''),
                    COUNT(*),
                    SUM(CASE WHEN http_status >= 400 THEN 1 ELSE 0 END),
                    SUM(input_tokens),
                    SUM(output_tokens),
                    SUM(cache_read_input_tokens),
                    SUM(cache_creation_input_tokens),
                    SUM(cost_nanodollars)
             FROM llm_events
             WHERE ts_ns >= ?2 AND ts_ns < ?3
             GROUP BY COALESCE(user_id, ''), COALESCE(api_key_id, ''),
                      COALESCE(provider, ''), COALESCE(model, '')",
            params![day_epoch, from_ns, to_ns],
        )?;

        tx.commit()?;
        Ok(())
    }

    // ── Aggregation queries (for Obol integration) ─────────────────────

    /// Real-time quota check: cost + event count from `llm_events`.
    ///
    /// At least one of `user_id` or `api_key_id` must be `Some`.
    pub fn quota_summary(
        &self,
        user_id: Option<&str>,
        api_key_id: Option<&str>,
        from_ts_ns: i64,
    ) -> Result<QuotaSummary, StoreError> {
        let conn = self.read_conn()?;

        let mut sql = String::from(
            "SELECT COALESCE(SUM(cost_nanodollars),0), COUNT(*) FROM llm_events WHERE ts_ns >= ?1",
        );
        let mut bind_idx = 1usize;

        // Dynamic filter: user_id and/or api_key_id.
        if let Some(_uid) = user_id {
            bind_idx += 1;
            sql.push_str(&format!(" AND user_id = ?{bind_idx}"));
        }
        if let Some(_kid) = api_key_id {
            bind_idx += 1;
            sql.push_str(&format!(" AND api_key_id = ?{bind_idx}"));
        }

        let mut stmt = conn.prepare_cached(&sql)?;

        // Build params dynamically.
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::with_capacity(3);
        params_vec.push(Box::new(from_ts_ns));
        if let Some(uid) = user_id {
            params_vec.push(Box::new(uid.to_owned()));
        }
        if let Some(kid) = api_key_id {
            params_vec.push(Box::new(kid.to_owned()));
        }
        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|b| b.as_ref()).collect();

        let (cost, count) = stmt
            .query_row(params_ref.as_slice(), |r| Ok((r.get::<_, i64>(0)?, r.get::<_, i64>(1)?)))?;

        Ok(QuotaSummary { cost_nanodollars: cost, event_count: count })
    }

    /// Query pre-aggregated daily rollup rows.
    pub fn query_rollups(
        &self,
        user_id: Option<&str>,
        api_key_id: Option<&str>,
        from_day: i64,
        to_day: i64,
    ) -> Result<Vec<RollupRow>, StoreError> {
        let conn = self.read_conn()?;

        let mut sql = String::from(
            "SELECT day, user_id, api_key_id, provider, model, \
             event_count, error_count, input_tokens, output_tokens, \
             cache_read_input_tokens, cache_creation_input_tokens, cost_nanodollars \
             FROM daily_rollups WHERE day >= ?1 AND day <= ?2",
        );

        let mut bind_idx = 2usize;
        if user_id.is_some() {
            bind_idx += 1;
            sql.push_str(&format!(" AND user_id = ?{bind_idx}"));
        }
        if api_key_id.is_some() {
            bind_idx += 1;
            sql.push_str(&format!(" AND api_key_id = ?{bind_idx}"));
        }
        sql.push_str(" ORDER BY day ASC");

        let mut stmt = conn.prepare_cached(&sql)?;

        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::with_capacity(4);
        params_vec.push(Box::new(from_day));
        params_vec.push(Box::new(to_day));
        if let Some(uid) = user_id {
            params_vec.push(Box::new(uid.to_owned()));
        }
        if let Some(kid) = api_key_id {
            params_vec.push(Box::new(kid.to_owned()));
        }
        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|b| b.as_ref()).collect();

        let rows = stmt.query_map(params_ref.as_slice(), row_to_rollup)?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Aggregate stats from `daily_rollups`, optionally grouped by model.
    pub fn aggregate_stats(
        &self,
        user_id: Option<&str>,
        api_key_id: Option<&str>,
        from_day: i64,
        to_day: i64,
        provider_filter: Option<&str>,
        group_by_model: bool,
    ) -> Result<Vec<AggregateRow>, StoreError> {
        let conn = self.read_conn()?;

        let select_cols =
            if group_by_model { "provider, model" } else { "'' AS provider, '' AS model" };
        let mut sql = format!(
            "SELECT {select_cols}, \
             SUM(event_count), SUM(error_count), \
             SUM(input_tokens), SUM(output_tokens), \
             SUM(cache_read_input_tokens), SUM(cache_creation_input_tokens), \
             SUM(cost_nanodollars) \
             FROM daily_rollups WHERE day >= ?1 AND day <= ?2"
        );

        let mut bind_idx = 2usize;
        if user_id.is_some() {
            bind_idx += 1;
            sql.push_str(&format!(" AND user_id = ?{bind_idx}"));
        }
        if api_key_id.is_some() {
            bind_idx += 1;
            sql.push_str(&format!(" AND api_key_id = ?{bind_idx}"));
        }
        if provider_filter.is_some() {
            bind_idx += 1;
            sql.push_str(&format!(" AND provider = ?{bind_idx}"));
        }
        if group_by_model {
            sql.push_str(" GROUP BY provider, model ORDER BY SUM(cost_nanodollars) DESC");
        }

        let mut stmt = conn.prepare_cached(&sql)?;

        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::with_capacity(5);
        params_vec.push(Box::new(from_day));
        params_vec.push(Box::new(to_day));
        if let Some(uid) = user_id {
            params_vec.push(Box::new(uid.to_owned()));
        }
        if let Some(kid) = api_key_id {
            params_vec.push(Box::new(kid.to_owned()));
        }
        if let Some(prov) = provider_filter {
            params_vec.push(Box::new(prov.to_owned()));
        }
        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|b| b.as_ref()).collect();

        let rows = stmt.query_map(params_ref.as_slice(), row_to_aggregate)?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Delete events older than `older_than_ns` and clean up blobs with
    /// refcount reaching 0.  Uses set-based SQL — O(3) statements
    /// regardless of event count.
    pub fn gc_expired(&self, older_than_ns: i64) -> Result<GcStats, StoreError> {
        let conn = self.write_conn()?;
        let tx = conn.unchecked_transaction()?;

        // Bulk-decrement refcounts for all components of expiring events.
        tx.execute(
            "UPDATE payload_blobs SET refcount = refcount - (
                SELECT COUNT(*) FROM event_components ec
                JOIN llm_events e ON ec.event_id = e.id
                WHERE e.ts_ns < ?1 AND ec.blob_sha256 = payload_blobs.sha256
             )
             WHERE sha256 IN (
                SELECT ec.blob_sha256 FROM event_components ec
                JOIN llm_events e ON ec.event_id = e.id
                WHERE e.ts_ns < ?1
             )",
            [older_than_ns],
        )?;

        // Delete component links.
        tx.execute(
            "DELETE FROM event_components WHERE event_id IN (
                SELECT id FROM llm_events WHERE ts_ns < ?1
             )",
            [older_than_ns],
        )?;

        // Delete the events themselves.
        let events_deleted =
            tx.execute("DELETE FROM llm_events WHERE ts_ns < ?1", [older_than_ns])?;

        // Remove orphaned blobs.
        let blobs_deleted = tx.execute("DELETE FROM payload_blobs WHERE refcount <= 0", [])?;

        tx.commit()?;
        Ok(GcStats { events_deleted, blobs_deleted })
    }

    /// Get the refcount for a blob by its SHA-256.
    pub fn blob_refcount(&self, sha: &[u8; 32]) -> Result<Option<i64>, StoreError> {
        let conn = self.read_conn()?;
        conn.query_row("SELECT refcount FROM payload_blobs WHERE sha256 = ?1", [&sha[..]], |r| {
            r.get(0)
        })
        .optional()
        .map_err(StoreError::from)
    }

    /// Count total blob rows.
    pub fn blob_count(&self) -> Result<usize, StoreError> {
        let conn = self.read_conn()?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM payload_blobs", [], |r| r.get(0))?;
        #[allow(clippy::cast_sign_loss)]
        Ok(count as usize)
    }

    /// Total stored bytes (compressed) across all blobs.
    pub fn total_compressed_bytes(&self) -> Result<i64, StoreError> {
        let conn = self.read_conn()?;
        let total: i64 = conn.query_row(
            "SELECT COALESCE(SUM(compressed_size), 0) FROM payload_blobs",
            [],
            |r| r.get(0),
        )?;
        Ok(total)
    }

    /// Total uncompressed bytes across all blobs (for compression ratio).
    pub fn total_uncompressed_bytes(&self) -> Result<i64, StoreError> {
        let conn = self.read_conn()?;
        let total: i64 = conn.query_row(
            "SELECT COALESCE(SUM(uncompressed_size), 0) FROM payload_blobs",
            [],
            |r| r.get(0),
        )?;
        Ok(total)
    }

    /// Run a WAL checkpoint to truncate the write-ahead log.
    ///
    /// Prevents the WAL file from growing unbounded under sustained
    /// write load. Safe to call while readers are active.
    pub fn wal_checkpoint(&self) -> Result<(), StoreError> {
        let conn = self.write_conn()?;
        conn.execute_batch("PRAGMA wal_checkpoint(TRUNCATE);")?;
        Ok(())
    }
}

struct PreparedBlob {
    sha: [u8; 32],
    component_type: &'static str,
    /// Provider that produced this blob (for correct dict_id tagging).
    provider_key: &'static str,
    uncompressed_len: usize,
    compressed: Vec<u8>,
    /// Whether a trained dict was used for compression.
    used_dict: bool,
}

fn sha256_bytes(data: &[u8]) -> [u8; 32] {
    let mut hasher = Sha256::new();
    hasher.update(data);
    hasher.finalize().into()
}

/// Column indices for SELECT * FROM llm_events.  Using positional access
/// avoids a per-column string lookup on every row.
mod col {
    pub const ID: usize = 0;
    pub const TS_NS: usize = 1;
    pub const USER_ID: usize = 2;
    pub const API_KEY_ID: usize = 3;
    pub const ORG_ID: usize = 4;
    pub const PROJECT_ID: usize = 5;
    pub const ROUTE_ID: usize = 6;
    pub const PROVIDER: usize = 7;
    pub const MODEL: usize = 8;
    pub const MODEL_FAMILY: usize = 9;
    pub const ENDPOINT: usize = 10;
    pub const METHOD: usize = 11;
    pub const HTTP_STATUS: usize = 12;
    pub const INPUT_TOKENS: usize = 13;
    pub const OUTPUT_TOKENS: usize = 14;
    pub const CACHE_READ: usize = 15;
    pub const CACHE_CREATION: usize = 16;
    pub const REASONING: usize = 17;
    pub const AUDIO_IN: usize = 18;
    pub const AUDIO_OUT: usize = 19;
    pub const IMAGE: usize = 20;
    pub const TOOL_USE: usize = 21;
    pub const COST: usize = 22;
    pub const TTFT: usize = 23;
    pub const TOTAL_MS: usize = 24;
    pub const CLOSE_MS: usize = 25;
    pub const STREAMING: usize = 26;
    pub const TOOL_CALLS: usize = 27;
    pub const REASONING_FLAG: usize = 28;
    pub const INCOMPLETE: usize = 29;
    pub const ERR_TYPE: usize = 30;
    pub const ERR_MSG: usize = 31;
    pub const REQ_SHA: usize = 32;
    pub const RESP_SHA: usize = 33;
    // 34, 35 = request_blob_id, response_blob_id (not read by row_to_event)
    pub const CLIENT_IP: usize = 36;
    pub const USER_AGENT: usize = 37;
    pub const REQ_ID: usize = 38;
    pub const TRACE_ID: usize = 39;
    pub const SOURCE: usize = 40;
    pub const INGESTED_AT: usize = 41;
    pub const METADATA_JSON: usize = 42;
}

/// Column indices for the narrow `query_summary` SELECT.
mod slim {
    pub const ID: usize = 0;
    pub const TS_NS: usize = 1;
    pub const USER_ID: usize = 2;
    pub const API_KEY_ID: usize = 3;
    pub const PROVIDER: usize = 4;
    pub const MODEL: usize = 5;
    pub const ENDPOINT: usize = 6;
    pub const HTTP_STATUS: usize = 7;
    pub const INPUT_TOKENS: usize = 8;
    pub const OUTPUT_TOKENS: usize = 9;
    pub const CACHE_READ: usize = 10;
    pub const CACHE_CREATION: usize = 11;
    pub const REASONING: usize = 12;
    pub const COST: usize = 13;
    pub const TTFT: usize = 14;
    pub const TOTAL_MS: usize = 15;
    pub const STREAMING: usize = 16;
    pub const SOURCE: usize = 17;
    pub const ERROR_TYPE: usize = 18;
    pub const METADATA_JSON: usize = 19;
}

fn row_to_summary(row: &rusqlite::Row<'_>) -> rusqlite::Result<EventSummary> {
    let id_bytes: Vec<u8> = row.get(slim::ID)?;
    let id_arr: [u8; 16] = id_bytes.try_into().map_err(|_| {
        rusqlite::Error::InvalidColumnType(0, "id".into(), rusqlite::types::Type::Blob)
    })?;

    Ok(EventSummary {
        id: keplor_core::EventId(ulid::Ulid::from_bytes(id_arr)),
        ts_ns: row.get(slim::TS_NS)?,
        user_id: row.get(slim::USER_ID)?,
        api_key_id: row.get(slim::API_KEY_ID)?,
        provider: row.get(slim::PROVIDER)?,
        model: row.get(slim::MODEL)?,
        endpoint: row.get(slim::ENDPOINT)?,
        http_status: row.get::<_, Option<i64>>(slim::HTTP_STATUS)?.map(|s| s as u16),
        input_tokens: row.get::<_, i64>(slim::INPUT_TOKENS)? as u32,
        output_tokens: row.get::<_, i64>(slim::OUTPUT_TOKENS)? as u32,
        cache_read_input_tokens: row.get::<_, i64>(slim::CACHE_READ)? as u32,
        cache_creation_input_tokens: row.get::<_, i64>(slim::CACHE_CREATION)? as u32,
        reasoning_tokens: row.get::<_, i64>(slim::REASONING)? as u32,
        cost_nanodollars: row.get(slim::COST)?,
        ttft_ms: row.get::<_, Option<i64>>(slim::TTFT)?.map(|v| v as u32),
        total_ms: row.get::<_, i64>(slim::TOTAL_MS)? as u32,
        streaming: row.get::<_, i64>(slim::STREAMING)? != 0,
        source: row.get(slim::SOURCE)?,
        error_type: row.get(slim::ERROR_TYPE)?,
        metadata_json: row.get(slim::METADATA_JSON)?,
    })
}

fn row_to_event(row: &rusqlite::Row<'_>) -> rusqlite::Result<LlmEvent> {
    let id_bytes: Vec<u8> = row.get(col::ID)?;
    let id_arr: [u8; 16] = id_bytes.try_into().map_err(|_| {
        rusqlite::Error::InvalidColumnType(0, "id".into(), rusqlite::types::Type::Blob)
    })?;

    let provider_str: String = row.get(col::PROVIDER)?;
    let method_str: String = row.get(col::METHOD)?;

    let error_type: Option<String> = row.get(col::ERR_TYPE)?;
    let error_message: Option<String> = row.get(col::ERR_MSG)?;

    let flags = {
        let mut f = EventFlags::empty();
        if row.get::<_, i64>(col::STREAMING)? != 0 {
            f |= EventFlags::STREAMING;
        }
        if row.get::<_, i64>(col::TOOL_CALLS)? != 0 {
            f |= EventFlags::TOOL_CALLS;
        }
        if row.get::<_, i64>(col::REASONING_FLAG)? != 0 {
            f |= EventFlags::REASONING;
        }
        if row.get::<_, i64>(col::INCOMPLETE)? != 0 {
            f |= EventFlags::STREAM_INCOMPLETE;
        }
        f
    };

    let request_sha256: Vec<u8> = row.get(col::REQ_SHA)?;
    let response_sha256: Vec<u8> = row.get(col::RESP_SHA)?;

    let client_ip_str: Option<String> = row.get(col::CLIENT_IP)?;
    let trace_id_str: Option<String> = row.get(col::TRACE_ID)?;

    Ok(LlmEvent {
        id: EventId(ulid::Ulid::from_bytes(id_arr)),
        ts_ns: row.get(col::TS_NS)?,
        user_id: row.get::<_, Option<String>>(col::USER_ID)?.map(|s| s.as_str().into()),
        api_key_id: row.get::<_, Option<String>>(col::API_KEY_ID)?.map(|s| s.as_str().into()),
        org_id: row.get::<_, Option<String>>(col::ORG_ID)?.map(|s| s.as_str().into()),
        project_id: row.get::<_, Option<String>>(col::PROJECT_ID)?.map(|s| s.as_str().into()),
        route_id: row.get::<_, Option<String>>(col::ROUTE_ID)?.unwrap_or_default().as_str().into(),
        provider: Provider::from_id_key(&provider_str),
        model: SmolStr::new(row.get::<_, String>(col::MODEL)?),
        model_family: row.get::<_, Option<String>>(col::MODEL_FAMILY)?.map(SmolStr::new),
        endpoint: SmolStr::new(row.get::<_, String>(col::ENDPOINT)?),
        method: http::Method::from_bytes(method_str.as_bytes()).unwrap_or(http::Method::POST),
        http_status: row.get::<_, Option<i64>>(col::HTTP_STATUS)?.map(|s| s as u16),
        usage: Usage {
            input_tokens: row.get::<_, i64>(col::INPUT_TOKENS)? as u32,
            output_tokens: row.get::<_, i64>(col::OUTPUT_TOKENS)? as u32,
            cache_read_input_tokens: row.get::<_, i64>(col::CACHE_READ)? as u32,
            cache_creation_input_tokens: row.get::<_, i64>(col::CACHE_CREATION)? as u32,
            reasoning_tokens: row.get::<_, i64>(col::REASONING)? as u32,
            audio_input_tokens: row.get::<_, i64>(col::AUDIO_IN)? as u32,
            audio_output_tokens: row.get::<_, i64>(col::AUDIO_OUT)? as u32,
            image_tokens: row.get::<_, i64>(col::IMAGE)? as u32,
            tool_use_tokens: row.get::<_, i64>(col::TOOL_USE)? as u32,
            ..Usage::default()
        },
        cost_nanodollars: row.get(col::COST)?,
        latency: Latencies {
            ttft_ms: row.get::<_, Option<i64>>(col::TTFT)?.map(|v| v as u32),
            total_ms: row.get::<_, i64>(col::TOTAL_MS)? as u32,
            time_to_close_ms: row.get::<_, Option<i64>>(col::CLOSE_MS)?.map(|v| v as u32),
        },
        flags,
        error: error_type.map(|t| error_from_stored(&t, error_message.as_deref())),
        request_sha256: request_sha256.try_into().unwrap_or([0u8; 32]),
        response_sha256: response_sha256.try_into().unwrap_or([0u8; 32]),
        client_ip: client_ip_str.and_then(|s| s.parse().ok()),
        user_agent: row.get::<_, Option<String>>(col::USER_AGENT)?.map(SmolStr::new),
        request_id: row.get::<_, Option<String>>(col::REQ_ID)?.map(SmolStr::new),
        trace_id: trace_id_str.and_then(|s| s.parse().ok()),
        source: row.get::<_, Option<String>>(col::SOURCE)?.map(SmolStr::new),
        ingested_at: row.get::<_, Option<i64>>(col::INGESTED_AT)?.unwrap_or(0),
        metadata: row
            .get::<_, Option<String>>(col::METADATA_JSON)?
            .and_then(|s| serde_json::from_str(&s).ok()),
    })
}

fn error_type_str(e: &ProviderError) -> &'static str {
    match e {
        ProviderError::RateLimited { .. } => "rate_limited",
        ProviderError::InvalidRequest(_) => "invalid_request",
        ProviderError::AuthFailed => "auth_failed",
        ProviderError::ContextLengthExceeded { .. } => "context_length_exceeded",
        ProviderError::ContentFiltered { .. } => "content_filtered",
        ProviderError::UpstreamTimeout => "upstream_timeout",
        ProviderError::UpstreamUnavailable => "upstream_unavailable",
        ProviderError::Other { .. } => "other",
    }
}

fn row_to_rollup(row: &rusqlite::Row<'_>) -> rusqlite::Result<RollupRow> {
    Ok(RollupRow {
        day: row.get(0)?,
        user_id: row.get(1)?,
        api_key_id: row.get(2)?,
        provider: row.get(3)?,
        model: row.get(4)?,
        event_count: row.get(5)?,
        error_count: row.get(6)?,
        input_tokens: row.get(7)?,
        output_tokens: row.get(8)?,
        cache_read_input_tokens: row.get(9)?,
        cache_creation_input_tokens: row.get(10)?,
        cost_nanodollars: row.get(11)?,
    })
}

fn row_to_aggregate(row: &rusqlite::Row<'_>) -> rusqlite::Result<AggregateRow> {
    Ok(AggregateRow {
        provider: row.get(0)?,
        model: row.get(1)?,
        event_count: row.get(2)?,
        error_count: row.get(3)?,
        input_tokens: row.get(4)?,
        output_tokens: row.get(5)?,
        cache_read_input_tokens: row.get(6)?,
        cache_creation_input_tokens: row.get(7)?,
        cost_nanodollars: row.get(8)?,
    })
}

fn error_from_stored(kind: &str, message: Option<&str>) -> ProviderError {
    let msg = SmolStr::new(message.unwrap_or(""));
    match kind {
        "rate_limited" => ProviderError::RateLimited { retry_after: None },
        "invalid_request" => ProviderError::InvalidRequest(msg.to_string()),
        "auth_failed" => ProviderError::AuthFailed,
        "context_length_exceeded" => ProviderError::ContextLengthExceeded { limit: 0 },
        "content_filtered" => ProviderError::ContentFiltered { reason: msg },
        "upstream_timeout" => ProviderError::UpstreamTimeout,
        "upstream_unavailable" => ProviderError::UpstreamUnavailable,
        _ => ProviderError::Other { status: 0, message: msg },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use keplor_core::*;

    fn test_event() -> LlmEvent {
        LlmEvent {
            id: EventId::new(),
            ts_ns: 1_700_000_000_000_000_000,
            user_id: Some(UserId::from("user_1")),
            api_key_id: Some(ApiKeyId::from("key_1")),
            org_id: None,
            project_id: None,
            route_id: RouteId::from("chat"),
            provider: Provider::OpenAI,
            model: SmolStr::new("gpt-4o"),
            model_family: Some(SmolStr::new("gpt-4")),
            endpoint: SmolStr::new("/v1/chat/completions"),
            method: http::Method::POST,
            http_status: Some(200),
            usage: Usage { input_tokens: 100, output_tokens: 50, ..Usage::default() },
            cost_nanodollars: 750_000,
            latency: Latencies { ttft_ms: Some(25), total_ms: 300, time_to_close_ms: None },
            flags: EventFlags::STREAMING,
            error: None,
            request_sha256: [0u8; 32],
            response_sha256: [0u8; 32],
            client_ip: Some("127.0.0.1".parse().unwrap()),
            user_agent: Some(SmolStr::new("test/1.0")),
            request_id: Some(SmolStr::new("req_abc")),
            trace_id: None,
            source: None,
            ingested_at: 0,
            metadata: None,
        }
    }

    fn test_req_body() -> Bytes {
        Bytes::from(
            r#"{"model":"gpt-4o","messages":[{"role":"system","content":"You are helpful."},{"role":"user","content":"Hello!"}]}"#,
        )
    }

    fn test_resp_body() -> Bytes {
        Bytes::from(
            r#"{"id":"chatcmpl-abc","choices":[{"message":{"role":"assistant","content":"Hi there!"}}],"usage":{"prompt_tokens":100,"completion_tokens":50}}"#,
        )
    }

    #[test]
    fn round_trip_append_get() {
        let store = Store::open_in_memory().unwrap();
        let event = test_event();
        let req = test_req_body();
        let resp = test_resp_body();

        let id = store.append_event(&event, &req, &resp).unwrap();

        let loaded = store.get_event(&id).unwrap().expect("event should exist");
        assert_eq!(loaded.id, event.id);
        assert_eq!(loaded.model, "gpt-4o");
        assert_eq!(loaded.usage.input_tokens, 100);
        assert_eq!(loaded.usage.output_tokens, 50);
        assert_eq!(loaded.cost_nanodollars, 750_000);
        assert_eq!(loaded.latency.ttft_ms, Some(25));
        assert!(loaded.flags.contains(EventFlags::STREAMING));
        assert_eq!(loaded.user_agent.as_deref(), Some("test/1.0"));

        let sys = store.get_component(&id, "system_prompt").unwrap();
        assert!(sys.is_some(), "system_prompt component should exist");

        let msgs = store.get_component(&id, "messages").unwrap();
        assert!(msgs.is_some(), "messages component should exist");

        let response = store.get_component(&id, "response").unwrap();
        assert!(response.is_some(), "response component should exist");
        assert_eq!(response.unwrap(), resp);
    }

    #[test]
    fn dedup_shared_system_prompt() {
        let store = Store::open_in_memory().unwrap();

        // Use canonical serde_json output — `split_request` assumes the
        // bytes come from `serde_json::to_vec` (as the pipeline produces).
        let v1: serde_json::Value = serde_json::from_str(
            r#"{"messages":[{"role":"system","content":"Be helpful."},{"role":"user","content":"A"}]}"#,
        ).unwrap();
        let v2: serde_json::Value = serde_json::from_str(
            r#"{"messages":[{"role":"system","content":"Be helpful."},{"role":"user","content":"B"}]}"#,
        ).unwrap();

        let e1 = test_event();
        let req1 = Bytes::from(serde_json::to_vec(&v1).unwrap());

        let mut e2 = test_event();
        e2.id = EventId::new();
        e2.ts_ns += 1;
        let req2 = Bytes::from(serde_json::to_vec(&v2).unwrap());

        let resp = test_resp_body();

        store.append_event(&e1, &req1, &resp).unwrap();
        store.append_event(&e2, &req2, &resp).unwrap();

        let sp_sha = sha256_bytes(
            &serde_json::to_vec(&serde_json::json!([{"role":"system","content":"Be helpful."}]))
                .unwrap(),
        );
        let rc = store.blob_refcount(&sp_sha).unwrap();
        assert_eq!(rc, Some(2), "system_prompt blob should have refcount=2");
    }

    #[test]
    fn gc_deletes_events_and_orphan_blobs() {
        let store = Store::open_in_memory().unwrap();

        let mut e1 = test_event();
        e1.ts_ns = 1_000;
        let mut e2 = test_event();
        e2.id = EventId::new();
        e2.ts_ns = 2_000;

        let req = Bytes::from(
            r#"{"messages":[{"role":"system","content":"Be helpful."},{"role":"user","content":"X"}]}"#,
        );
        let resp = test_resp_body();

        store.append_event(&e1, &req, &resp).unwrap();
        store.append_event(&e2, &req, &resp).unwrap();

        let stats = store.gc_expired(1500).unwrap();
        assert_eq!(stats.events_deleted, 1);
        assert_eq!(stats.blobs_deleted, 0);

        let stats = store.gc_expired(3000).unwrap();
        assert_eq!(stats.events_deleted, 1);
        assert!(stats.blobs_deleted > 0);

        let blobs_after = store.blob_count().unwrap();
        assert_eq!(blobs_after, 0, "all blobs should be gone");
    }

    #[test]
    fn query_with_user_filter() {
        let store = Store::open_in_memory().unwrap();

        let mut e1 = test_event();
        e1.user_id = Some(UserId::from("alice"));
        let mut e2 = test_event();
        e2.id = EventId::new();
        e2.user_id = Some(UserId::from("bob"));
        e2.ts_ns += 1;

        let req = test_req_body();
        let resp = test_resp_body();

        store.append_event(&e1, &req, &resp).unwrap();
        store.append_event(&e2, &req, &resp).unwrap();

        let filter = EventFilter { user_id: Some(SmolStr::new("alice")), ..Default::default() };
        let results = store.query(&filter, 100, None).unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].user_id.as_ref().unwrap().as_str(), "alice");
    }

    #[test]
    fn get_nonexistent_event_returns_none() {
        let store = Store::open_in_memory().unwrap();
        let result = store.get_event(&EventId::new()).unwrap();
        assert!(result.is_none());
    }

    #[test]
    fn query_summary_returns_correct_fields() {
        let store = Store::open_in_memory().unwrap();
        let event = test_event();
        let req = test_req_body();
        let resp = test_resp_body();

        store.append_event(&event, &req, &resp).unwrap();

        let filter = EventFilter { user_id: Some(SmolStr::new("user_1")), ..Default::default() };
        let results = store.query_summary(&filter, 10, None).unwrap();
        assert_eq!(results.len(), 1);

        let s = &results[0];
        assert_eq!(s.ts_ns, event.ts_ns);
        assert_eq!(s.provider, "openai");
        assert_eq!(s.model, "gpt-4o");
        assert_eq!(s.input_tokens, 100);
        assert_eq!(s.output_tokens, 50);
        assert_eq!(s.cost_nanodollars, 750_000);
        assert_eq!(s.total_ms, 300);
        assert_eq!(s.ttft_ms, Some(25));
        assert!(s.streaming);
    }

    #[test]
    fn train_dict_from_samples() {
        let mut store = Store::open_in_memory().unwrap();

        // Insert events with 20 distinct system prompts (need >= 10 unique blobs).
        for i in 0..20 {
            let mut event = test_event();
            event.id = EventId::new();
            event.ts_ns += i as i64;

            let sys = format!("You are assistant variant {i}. Help the user with task {i}.");
            let v: serde_json::Value = serde_json::from_str(&format!(
                r#"{{"messages":[{{"role":"system","content":{sys_json}}},{{"role":"user","content":"Q{i}"}}]}}"#,
                sys_json = serde_json::to_string(&sys).unwrap()
            ))
            .unwrap();
            let req = Bytes::from(serde_json::to_vec(&v).unwrap());
            let resp = test_resp_body();
            store.append_event(&event, &req, &resp).unwrap();
        }

        let result = store.train_dict("openai", "system_prompt", 100, 4096);
        assert!(result.is_ok());
        let dict_id = result.unwrap();
        assert!(dict_id.is_some(), "should have enough samples to train");
        assert_eq!(dict_id.unwrap(), "openai_system_prompt");
    }
}
