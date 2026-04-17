//! [`Store`] — the local SQLite-backed storage engine.

use std::path::Path;
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

/// Local SQLite store for LLM events and their payload blobs.
///
/// Thread-safe: holds a `Mutex<Connection>`.
pub struct Store {
    conn: Mutex<Connection>,
    coder: ZstdCoder,
}

impl std::fmt::Debug for Store {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Store").finish_non_exhaustive()
    }
}

impl Store {
    /// Open (or create) a store at the given path.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let conn = Connection::open(path)?;
        Self::init(conn)
    }

    /// Open an in-memory store (for tests).
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let conn = Connection::open_in_memory()?;
        Self::init(conn)
    }

    fn init(conn: Connection) -> Result<Self, StoreError> {
        migrations::apply_pragmas(&conn)?;
        migrations::migrate(&conn)?;
        Ok(Self { conn: Mutex::new(conn), coder: ZstdCoder::new() })
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

        let conn = self.conn.lock().map_err(|e| StoreError::Compression(e.to_string()))?;
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
                source, ingested_at
             ) VALUES(
                ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                ?8, ?9, ?10, ?11, ?12, ?13,
                ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22,
                ?23, ?24, ?25, ?26,
                ?27, ?28, ?29, ?30,
                ?31, ?32,
                ?33, ?34, ?35, ?36,
                ?37, ?38, ?39, ?40,
                ?41, ?42
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
                    let compressed = self.coder.compress(&comp.data, None)?;
                    unique_blobs.push(PreparedBlob {
                        sha,
                        component_type: comp.kind.as_str(),
                        uncompressed_len: comp.data.len(),
                        compressed,
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
            });
        }

        // ── Lock + single transaction ─────────────────────────────────
        let conn = self.conn.lock().map_err(|e| StoreError::Compression(e.to_string()))?;
        let tx = conn.unchecked_transaction()?;

        // Insert unique blobs with ON CONFLICT for cross-batch dedup.
        {
            let mut blob_stmt = tx.prepare_cached(
                "INSERT INTO payload_blobs(
                    sha256, component_type, provider, compression, dict_id,
                    uncompressed_size, compressed_size, refcount, hit_count,
                    data, first_seen_at
                 ) VALUES(?1, ?2, ?3, 'zstd_raw', NULL, ?4, ?5, 1, 0, ?6, strftime('%s','now'))
                 ON CONFLICT(sha256) DO UPDATE SET
                    refcount = refcount + 1,
                    hit_count = hit_count + 1",
            )?;

            // First pass: insert each unique blob once.
            for blob in &unique_blobs {
                blob_stmt.execute(params![
                    &blob.sha[..],
                    blob.component_type,
                    batch_events[0].provider_key,
                    blob.uncompressed_len as i64,
                    blob.compressed.len() as i64,
                    &blob.compressed[..],
                ])?;
            }

            // Second pass: bump refcount for intra-batch duplicates.
            let mut bump_stmt = tx.prepare_cached(
                "UPDATE payload_blobs SET refcount = refcount + 1, hit_count = hit_count + 1
                 WHERE sha256 = ?1",
            )?;
            // Total references minus unique insertions = extra bumps needed.
            let mut ref_counts: std::collections::HashMap<[u8; 32], usize> =
                std::collections::HashMap::with_capacity(unique_blobs.len());
            for be in &batch_events {
                for &(_, sha) in &be.component_links {
                    *ref_counts.entry(sha).or_insert(0) += 1;
                }
            }
            for (sha, count) in &ref_counts {
                for _ in 1..*count {
                    bump_stmt.execute([&sha[..]])?;
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
                    source, ingested_at
                 ) VALUES(
                    ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                    ?8, ?9, ?10, ?11, ?12, ?13,
                    ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22,
                    ?23, ?24, ?25, ?26,
                    ?27, ?28, ?29, ?30,
                    ?31, ?32,
                    ?33, ?34, ?35, ?36,
                    ?37, ?38, ?39, ?40,
                    ?41, ?42
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
                    e.client_ip.map(|ip| ip.to_string()),
                    e.user_agent.as_deref(),
                    e.request_id.as_deref(),
                    e.trace_id.map(|t| t.to_string()),
                    e.source.as_deref(),
                    e.ingested_at,
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
        let compressed = self.coder.compress(data, None)?;
        conn.execute(
            "INSERT INTO payload_blobs(
                sha256, component_type, provider, compression, dict_id,
                uncompressed_size, compressed_size, refcount, hit_count,
                data, first_seen_at
             ) VALUES(?1, ?2, ?3, 'zstd_raw', NULL, ?4, ?5, 1, 0, ?6, strftime('%s','now'))
             ON CONFLICT(sha256) DO UPDATE SET
                refcount = refcount + 1,
                hit_count = hit_count + 1",
            params![
                &sha[..],
                component_type,
                provider,
                data.len() as i64,
                compressed.len() as i64,
                &compressed[..],
            ],
        )?;
        Ok(())
    }

    /// Retrieve an event by id.
    pub fn get_event(&self, id: &EventId) -> Result<Option<LlmEvent>, StoreError> {
        let conn = self.conn.lock().map_err(|e| StoreError::Compression(e.to_string()))?;
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
        let conn = self.conn.lock().map_err(|e| StoreError::Compression(e.to_string()))?;
        let id_bytes = event_id.as_ulid().to_bytes();

        let compressed: Option<Vec<u8>> = conn
            .query_row(
                "SELECT pb.data FROM event_components ec
                 JOIN payload_blobs pb ON pb.sha256 = ec.blob_sha256
                 WHERE ec.event_id = ?1 AND ec.component_type = ?2",
                params![&id_bytes[..], component_type],
                |r| r.get(0),
            )
            .optional()?;

        let Some(compressed) = compressed else {
            return Ok(None);
        };

        let decompressed = self.coder.decompress(&compressed, None)?;
        Ok(Some(Bytes::from(decompressed)))
    }

    /// Query events with filters and cursor-based pagination.
    pub fn query(
        &self,
        filter: &EventFilter,
        limit: u32,
        cursor: Option<Cursor>,
    ) -> Result<Vec<LlmEvent>, StoreError> {
        let conn = self.conn.lock().map_err(|e| StoreError::Compression(e.to_string()))?;

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

    /// Roll up a day's events into `daily_rollups`.
    pub fn rollup_day(&self, day_epoch: i64) -> Result<(), StoreError> {
        let conn = self.conn.lock().map_err(|e| StoreError::Compression(e.to_string()))?;
        let next_day = day_epoch + 86400;

        conn.execute(
            "INSERT OR REPLACE INTO daily_rollups(day, user_id, api_key_id, model,
                event_count, input_tokens, output_tokens, cost_nanodollars)
             SELECT ?1, user_id, api_key_id, model,
                    COUNT(*), SUM(input_tokens), SUM(output_tokens), SUM(cost_nanodollars)
             FROM llm_events
             WHERE ts_ns >= ?2 AND ts_ns < ?3
             GROUP BY user_id, api_key_id, model",
            params![day_epoch, day_epoch * 1_000_000_000i64, next_day * 1_000_000_000i64],
        )?;
        Ok(())
    }

    /// Delete events older than `older_than_ns` and clean up blobs with
    /// refcount reaching 0.  Uses set-based SQL — O(3) statements
    /// regardless of event count.
    pub fn gc_expired(&self, older_than_ns: i64) -> Result<GcStats, StoreError> {
        let conn = self.conn.lock().map_err(|e| StoreError::Compression(e.to_string()))?;
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
        let conn = self.conn.lock().map_err(|e| StoreError::Compression(e.to_string()))?;
        conn.query_row("SELECT refcount FROM payload_blobs WHERE sha256 = ?1", [&sha[..]], |r| {
            r.get(0)
        })
        .optional()
        .map_err(StoreError::from)
    }

    /// Count total blob rows.
    pub fn blob_count(&self) -> Result<usize, StoreError> {
        let conn = self.conn.lock().map_err(|e| StoreError::Compression(e.to_string()))?;
        let count: i64 = conn.query_row("SELECT COUNT(*) FROM payload_blobs", [], |r| r.get(0))?;
        #[allow(clippy::cast_sign_loss)]
        Ok(count as usize)
    }

    /// Total stored bytes (compressed) across all blobs.
    pub fn total_compressed_bytes(&self) -> Result<i64, StoreError> {
        let conn = self.conn.lock().map_err(|e| StoreError::Compression(e.to_string()))?;
        let total: i64 = conn.query_row(
            "SELECT COALESCE(SUM(compressed_size), 0) FROM payload_blobs",
            [],
            |r| r.get(0),
        )?;
        Ok(total)
    }

    /// Total uncompressed bytes across all blobs (for compression ratio).
    pub fn total_uncompressed_bytes(&self) -> Result<i64, StoreError> {
        let conn = self.conn.lock().map_err(|e| StoreError::Compression(e.to_string()))?;
        let total: i64 = conn.query_row(
            "SELECT COALESCE(SUM(uncompressed_size), 0) FROM payload_blobs",
            [],
            |r| r.get(0),
        )?;
        Ok(total)
    }
}

struct PreparedBlob {
    sha: [u8; 32],
    component_type: &'static str,
    uncompressed_len: usize,
    compressed: Vec<u8>,
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

        let e1 = test_event();
        let req1 = Bytes::from(
            r#"{"messages":[{"role":"system","content":"Be helpful."},{"role":"user","content":"A"}]}"#,
        );

        let mut e2 = test_event();
        e2.id = EventId::new();
        e2.ts_ns += 1;
        let req2 = Bytes::from(
            r#"{"messages":[{"role":"system","content":"Be helpful."},{"role":"user","content":"B"}]}"#,
        );

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
}
