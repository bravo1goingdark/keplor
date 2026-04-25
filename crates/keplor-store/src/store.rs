//! [`Store`] — the local SQLite-backed storage engine.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use rusqlite::{params, Connection, OptionalExtension};
use smol_str::SmolStr;

use keplor_core::{EventFlags, EventId, Latencies, LlmEvent, Provider, ProviderError, Usage};

use crate::error::StoreError;
use crate::filter::{Cursor, EventFilter};
use crate::migrations;
use crate::types::{AggregateRow, ArchiveManifest, EventSummary, GcStats, QuotaSummary, RollupRow};

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

/// Local SQLite store for LLM events.
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

        let mut read_pool = Vec::with_capacity(pool_size);
        for _ in 0..pool_size {
            let rc = opener.open()?;
            migrations::apply_pragmas(&rc)?;
            read_pool.push(Mutex::new(rc));
        }

        Ok(Self { write_conn: Mutex::new(write_conn), read_pool, read_idx: AtomicUsize::new(0) })
    }

    /// Acquire a read connection from the pool (round-robin).
    fn read_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, StoreError> {
        let idx = self.read_idx.fetch_add(1, Ordering::Relaxed) % self.read_pool.len();
        self.read_pool[idx].lock().map_err(|e| StoreError::LockPoisoned(e.to_string()))
    }

    /// Acquire the write connection.
    fn write_conn(&self) -> Result<std::sync::MutexGuard<'_, Connection>, StoreError> {
        self.write_conn.lock().map_err(|e| StoreError::LockPoisoned(e.to_string()))
    }

    /// Append a single event.
    pub fn append_event(&self, event: &LlmEvent) -> Result<EventId, StoreError> {
        let conn = self.write_conn()?;
        Self::insert_event(&conn, event)?;
        Ok(event.id)
    }

    /// Append a batch of events in a single transaction.
    ///
    /// Prepared statements are reused across all events.
    pub fn append_batch(&self, events: &[LlmEvent]) -> Result<Vec<EventId>, StoreError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }

        let conn = self.write_conn()?;
        let tx = conn.unchecked_transaction()?;

        {
            let mut stmt = Self::prepare_insert(&tx)?;
            for event in events {
                Self::execute_insert(&mut stmt, event)?;
            }
        }

        tx.commit()?;
        Ok(events.iter().map(|e| e.id).collect())
    }

    /// Shared INSERT statement preparation.
    fn prepare_insert(conn: &Connection) -> Result<rusqlite::CachedStatement<'_>, StoreError> {
        Ok(conn.prepare_cached(
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
                source, ingested_at, metadata_json, tier
             ) VALUES(
                ?1, ?2, ?3, ?4, ?5, ?6, ?7,
                ?8, ?9, ?10, ?11, ?12, ?13,
                ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22,
                ?23, ?24, ?25, ?26,
                ?27, ?28, ?29, ?30,
                ?31, ?32,
                ?33, ?34, ?35, ?36,
                ?37, ?38, ?39, ?40,
                ?41, ?42, ?43, ?44
             )",
        )?)
    }

    /// Execute the prepared INSERT for a single event.
    fn execute_insert(
        stmt: &mut rusqlite::CachedStatement<'_>,
        event: &LlmEvent,
    ) -> Result<(), StoreError> {
        let error_type = event.error.as_ref().map(error_type_str);
        let error_message = event.error.as_ref().map(|e| e.to_string());
        let metadata_str =
            event.metadata.as_ref().map(|v| serde_json::to_string(v).unwrap_or_default());
        let client_ip_str = event.client_ip.map(|ip| ip.to_string());
        let trace_id_str = event.trace_id.map(|t| t.to_string());
        // Vestigial — dead columns kept for index stability.
        let zeroed_sha = [0u8; 32];

        stmt.execute(params![
            event.id.as_ulid().to_bytes().as_slice(),
            event.ts_ns,
            event.user_id.as_ref().map(|u| u.as_str()),
            event.api_key_id.as_ref().map(|a| a.as_str()),
            event.org_id.as_ref().map(|o| o.as_str()),
            event.project_id.as_ref().map(|p| p.as_str()),
            event.route_id.as_str(),
            event.provider.id_key(),
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
            &zeroed_sha[..], // request_sha256 (vestigial)
            &zeroed_sha[..], // response_sha256 (vestigial)
            None::<&[u8]>,   // request_blob_id (vestigial)
            None::<&[u8]>,   // response_blob_id (vestigial)
            client_ip_str,
            event.user_agent.as_deref(),
            event.request_id.as_deref(),
            trace_id_str,
            event.source.as_deref(),
            event.ingested_at,
            metadata_str.as_deref(),
            event.tier.as_str(),
        ])?;
        Ok(())
    }

    /// Insert a single event (used by `append_event`).
    fn insert_event(conn: &Connection, event: &LlmEvent) -> Result<(), StoreError> {
        let mut stmt = Self::prepare_insert(conn)?;
        Self::execute_insert(&mut stmt, event)
    }

    /// Retrieve an event by id.
    pub fn get_event(&self, id: &EventId) -> Result<Option<LlmEvent>, StoreError> {
        let conn = self.read_conn()?;
        let id_bytes = id.as_ulid().to_bytes();

        conn.query_row("SELECT * FROM llm_events WHERE id = ?1", [&id_bytes[..]], row_to_event)
            .optional()
            .map_err(StoreError::from)
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
        limit: u32,
        offset: u32,
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
        bind_idx += 1;
        sql.push_str(&format!(" LIMIT ?{bind_idx}"));
        bind_idx += 1;
        sql.push_str(&format!(" OFFSET ?{bind_idx}"));

        let mut stmt = conn.prepare_cached(&sql)?;

        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::with_capacity(6);
        params_vec.push(Box::new(from_day));
        params_vec.push(Box::new(to_day));
        if let Some(uid) = user_id {
            params_vec.push(Box::new(uid.to_owned()));
        }
        if let Some(kid) = api_key_id {
            params_vec.push(Box::new(kid.to_owned()));
        }
        params_vec.push(Box::new(limit));
        params_vec.push(Box::new(offset));
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
        limit: u32,
        offset: u32,
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
        bind_idx += 1;
        sql.push_str(&format!(" LIMIT ?{bind_idx}"));
        bind_idx += 1;
        sql.push_str(&format!(" OFFSET ?{bind_idx}"));

        let mut stmt = conn.prepare_cached(&sql)?;

        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::with_capacity(7);
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
        params_vec.push(Box::new(limit));
        params_vec.push(Box::new(offset));
        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|b| b.as_ref()).collect();

        let rows = stmt.query_map(params_ref.as_slice(), row_to_aggregate)?;
        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Delete events older than `older_than_ns`.
    pub fn gc_expired(&self, older_than_ns: i64) -> Result<GcStats, StoreError> {
        let conn = self.write_conn()?;
        let events_deleted =
            conn.execute("DELETE FROM llm_events WHERE ts_ns < ?1", [older_than_ns])?;
        Ok(GcStats { events_deleted, blobs_deleted: 0 })
    }

    /// Delete events for a specific tier older than `older_than_ns`.
    ///
    /// Used by the tiered GC loop: one call per configured retention tier.
    pub fn gc_tier(&self, tier: &str, older_than_ns: i64) -> Result<GcStats, StoreError> {
        let conn = self.write_conn()?;
        let events_deleted = conn.execute(
            "DELETE FROM llm_events WHERE ts_ns < ?1 AND tier = ?2",
            params![older_than_ns, tier],
        )?;
        Ok(GcStats { events_deleted, blobs_deleted: 0 })
    }

    /// Lightweight health probe — executes `SELECT 1` on a read connection.
    ///
    /// Returns `Ok(())` if the database is reachable, or a [`StoreError`] if
    /// the connection is broken or locked.
    pub fn health_probe(&self) -> Result<(), StoreError> {
        let conn = self.read_conn()?;
        conn.query_row("SELECT 1", [], |_| Ok(()))?;
        Ok(())
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

    /// Reclaim disk space by rebuilding the database file.
    ///
    /// SQLite does not return freed pages to the OS after DELETE.  This
    /// runs `VACUUM` which rewrites the entire database, compacting it.
    /// **Expensive** — blocks all writes for the duration.  Call after
    /// draining a large number of embedded blobs.
    pub fn vacuum(&self) -> Result<(), StoreError> {
        let conn = self.write_conn()?;
        conn.execute_batch("VACUUM;")?;
        Ok(())
    }

    /// Database file size in bytes (page_count × page_size).
    pub fn db_size_bytes(&self) -> Result<u64, StoreError> {
        let conn = self.read_conn()?;
        let page_count: i64 = conn.query_row("PRAGMA page_count", [], |r| r.get(0))?;
        let page_size: i64 = conn.query_row("PRAGMA page_size", [], |r| r.get(0))?;
        #[allow(clippy::cast_sign_loss)]
        Ok((page_count * page_size) as u64)
    }

    /// Delete a single event by ID.
    ///
    /// Returns `true` if the event existed and was deleted.
    pub fn delete_event(&self, id: &EventId) -> Result<bool, StoreError> {
        let conn = self.write_conn()?;
        let id_bytes = id.as_ulid().to_bytes();
        let deleted = conn.execute("DELETE FROM llm_events WHERE id = ?1", [&id_bytes[..]])?;
        Ok(deleted > 0)
    }

    /// Stream event summaries matching a filter without collecting into a
    /// `Vec`. Calls `callback` for each matching row. Used for bulk export.
    ///
    /// Unlike [`Store::query_summary`], this method has no result-set limit.
    pub fn export_events(
        &self,
        filter: &EventFilter,
        callback: &mut dyn FnMut(EventSummary),
    ) -> Result<(), StoreError> {
        // Reuse query_summary with u32::MAX — SQLite handles LIMIT
        // 4_294_967_295 efficiently (it's essentially unlimited).
        let events = self.query_summary(filter, u32::MAX, None)?;
        for e in events {
            callback(e);
        }
        Ok(())
    }

    // ── Archive support ──────────────────────────────────────────────────

    /// Roll up all days that have events in the given timestamp range.
    ///
    /// Called before archival to ensure `daily_rollups` data persists
    /// even after source events are deleted.
    pub fn rollup_days_for_range(&self, from_ns: i64, to_ns: i64) -> Result<(), StoreError> {
        let from_day = from_ns / 1_000_000_000 / 86400 * 86400;
        let to_day = to_ns / 1_000_000_000 / 86400 * 86400;

        let mut day = from_day;
        while day <= to_day {
            self.rollup_day(day)?;
            day += 86400;
        }
        Ok(())
    }

    /// Fetch all events older than `older_than_ns` for archival.
    ///
    /// Ordered by `user_id, ts_ns` so the caller can group efficiently.
    pub fn query_events_for_archive(
        &self,
        older_than_ns: i64,
    ) -> Result<Vec<LlmEvent>, StoreError> {
        let conn = self.read_conn()?;
        let mut stmt = conn
            .prepare_cached("SELECT * FROM llm_events WHERE ts_ns < ?1 ORDER BY user_id, ts_ns")?;
        let rows = stmt.query_map([older_than_ns], row_to_event)?;
        let mut events = Vec::new();
        for row in rows {
            events.push(row?);
        }
        Ok(events)
    }

    /// Record an archive manifest after a successful S3 upload.
    #[cfg(feature = "s3")]
    pub fn insert_archive_manifest(&self, m: &ArchiveManifest) -> Result<(), StoreError> {
        let conn = self.write_conn()?;
        conn.execute(
            "INSERT INTO archive_manifests(
                archive_id, user_id, day, s3_key, event_count,
                min_ts_ns, max_ts_ns, compressed_bytes, created_at
             ) VALUES(?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
            params![
                m.archive_id,
                m.user_id,
                m.day,
                m.s3_key,
                m.event_count as i64,
                m.min_ts_ns,
                m.max_ts_ns,
                m.compressed_bytes as i64,
                m.created_at,
            ],
        )?;
        Ok(())
    }

    /// Delete events by a list of primary keys.
    ///
    /// Used after confirmed S3 upload to remove archived events from SQLite.
    pub fn delete_events_by_ids(&self, ids: &[EventId]) -> Result<usize, StoreError> {
        if ids.is_empty() {
            return Ok(0);
        }
        let conn = self.write_conn()?;
        let tx = conn.unchecked_transaction()?;
        let mut total = 0usize;
        {
            let mut stmt = tx.prepare_cached("DELETE FROM llm_events WHERE id = ?1")?;
            for id in ids {
                total += stmt.execute([&id.as_ulid().to_bytes()[..]])?;
            }
        }
        tx.commit()?;
        Ok(total)
    }

    /// Check if any archived data exists for a given time range.
    pub fn has_archived_data(
        &self,
        from_ts_ns: Option<i64>,
        to_ts_ns: Option<i64>,
    ) -> Result<bool, StoreError> {
        let conn = self.read_conn()?;
        let (sql, params_vec): (String, Vec<Box<dyn rusqlite::types::ToSql>>) = match (
            from_ts_ns, to_ts_ns,
        ) {
            (Some(from), Some(to)) => (
                "SELECT 1 FROM archive_manifests WHERE max_ts_ns >= ?1 AND min_ts_ns <= ?2 LIMIT 1"
                    .to_owned(),
                vec![Box::new(from), Box::new(to)],
            ),
            (Some(from), None) => (
                "SELECT 1 FROM archive_manifests WHERE max_ts_ns >= ?1 LIMIT 1".to_owned(),
                vec![Box::new(from)],
            ),
            (None, Some(to)) => (
                "SELECT 1 FROM archive_manifests WHERE min_ts_ns <= ?1 LIMIT 1".to_owned(),
                vec![Box::new(to)],
            ),
            (None, None) => ("SELECT 1 FROM archive_manifests LIMIT 1".to_owned(), vec![]),
        };

        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|b| b.as_ref()).collect();
        let exists: bool =
            conn.query_row(&sql, params_ref.as_slice(), |_| Ok(true)).optional()?.unwrap_or(false);
        Ok(exists)
    }

    /// List archive manifests matching a filter.
    pub fn list_archives(
        &self,
        user_id: Option<&str>,
        from_ts_ns: Option<i64>,
        to_ts_ns: Option<i64>,
    ) -> Result<Vec<ArchiveManifest>, StoreError> {
        let conn = self.read_conn()?;

        let mut sql = String::from(
            "SELECT archive_id, user_id, day, s3_key, event_count, \
             min_ts_ns, max_ts_ns, compressed_bytes, created_at \
             FROM archive_manifests WHERE 1=1",
        );
        let mut params_vec: Vec<Box<dyn rusqlite::types::ToSql>> = Vec::with_capacity(3);
        let mut idx = 0usize;

        if let Some(uid) = user_id {
            idx += 1;
            sql.push_str(&format!(" AND user_id = ?{idx}"));
            params_vec.push(Box::new(uid.to_owned()));
        }
        if let Some(from) = from_ts_ns {
            idx += 1;
            sql.push_str(&format!(" AND max_ts_ns >= ?{idx}"));
            params_vec.push(Box::new(from));
        }
        if let Some(to) = to_ts_ns {
            idx += 1;
            sql.push_str(&format!(" AND min_ts_ns <= ?{idx}"));
            params_vec.push(Box::new(to));
        }
        sql.push_str(" ORDER BY min_ts_ns ASC");

        let params_ref: Vec<&dyn rusqlite::types::ToSql> =
            params_vec.iter().map(|b| b.as_ref()).collect();
        let mut stmt = conn.prepare_cached(&sql)?;
        let rows = stmt.query_map(params_ref.as_slice(), |r| {
            Ok(ArchiveManifest {
                archive_id: r.get(0)?,
                user_id: r.get(1)?,
                day: r.get(2)?,
                s3_key: r.get(3)?,
                event_count: r.get::<_, i64>(4)? as usize,
                min_ts_ns: r.get(5)?,
                max_ts_ns: r.get(6)?,
                compressed_bytes: r.get::<_, i64>(7)? as usize,
                created_at: r.get(8)?,
            })
        })?;

        let mut result = Vec::new();
        for row in rows {
            result.push(row?);
        }
        Ok(result)
    }

    /// Summary of all archived data (for the CLI `archive-status` command).
    pub fn archive_summary(&self) -> Result<(usize, usize, i64), StoreError> {
        let conn = self.read_conn()?;
        let (files, events, bytes): (i64, i64, i64) = conn.query_row(
            "SELECT COUNT(*), COALESCE(SUM(event_count), 0), COALESCE(SUM(compressed_bytes), 0) \
             FROM archive_manifests",
            [],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )?;
        Ok((files as usize, events as usize, bytes))
    }
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
    pub const TIER: usize = 43;
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
        tier: SmolStr::new(
            row.get::<_, Option<String>>(col::TIER)?.unwrap_or_else(|| "free".to_owned()),
        ),
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
            tier: SmolStr::new("free"),
        }
    }

    #[test]
    fn round_trip_append_get() {
        let store = Store::open_in_memory().unwrap();
        let event = test_event();

        let id = store.append_event(&event).unwrap();

        let loaded = store.get_event(&id).unwrap().expect("event should exist");
        assert_eq!(loaded.id, event.id);
        assert_eq!(loaded.model, "gpt-4o");
        assert_eq!(loaded.usage.input_tokens, 100);
        assert_eq!(loaded.usage.output_tokens, 50);
        assert_eq!(loaded.cost_nanodollars, 750_000);
        assert_eq!(loaded.latency.ttft_ms, Some(25));
        assert!(loaded.flags.contains(EventFlags::STREAMING));
        assert_eq!(loaded.user_agent.as_deref(), Some("test/1.0"));
    }

    #[test]
    fn gc_deletes_events() {
        let store = Store::open_in_memory().unwrap();

        let mut e1 = test_event();
        e1.ts_ns = 1_000;
        let mut e2 = test_event();
        e2.id = EventId::new();
        e2.ts_ns = 2_000;

        store.append_event(&e1).unwrap();
        store.append_event(&e2).unwrap();

        let stats = store.gc_expired(1500).unwrap();
        assert_eq!(stats.events_deleted, 1);
        assert_eq!(stats.blobs_deleted, 0);

        let stats = store.gc_expired(3000).unwrap();
        assert_eq!(stats.events_deleted, 1);
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

        store.append_event(&e1).unwrap();
        store.append_event(&e2).unwrap();

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

        store.append_event(&event).unwrap();

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
}
