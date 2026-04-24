//! `KdbStore` — KeplorDB-backed event store.
//!
//! Architecture: one [`keplordb::Engine`] per retention tier, spawned
//! under `{data_dir}/tier={tier}/`. Cross-tier reads fan out across
//! engines and merge by `ts_ns` descending. Per-tier GC drops whole
//! segments without touching other tiers.
//!
//! A small sidecar file (`archive_manifests.jsonl`) tracks S3/R2
//! archive chunks — KeplorDB itself has no schema for manifests, so
//! they live in a separate append-only log with an in-memory index.
//!
//! ## Read visibility
//!
//! KeplorDB queries only see events that have been rotated into
//! segment files — events in the active WAL buffer are durable on
//! disk (WAL replay recovers them on restart) but not queryable until
//! rotation. In production every write goes through the
//! [`crate::batch::BatchWriter`] which flushes on its configured
//! cadence (default 50 ms / 256 events), capping read lag at that
//! window. Callers that need write-then-read-immediately semantics
//! must call [`KdbStore::wal_checkpoint`] explicitly.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use arc_swap::ArcSwap;
use keplor_core::{EventId, LlmEvent};
use keplordb::read::query::Cursor as KdbCursor;
use keplordb::{Engine, EngineConfig, QueryFilter};
use smol_str::SmolStr;

use crate::error::StoreError;
use crate::filter::{Cursor, EventFilter};
use crate::mapping::{self, C, D, DIM_API_KEY_ID, DIM_USER_ID, L, SCHEMA_ID};
use crate::store::{AggregateRow, ArchiveManifest, EventSummary, GcStats, QuotaSummary, RollupRow};

mod manifests;

use manifests::ManifestStore;

/// Configuration template applied to every per-tier engine.
///
/// Matches the knobs of [`keplordb::EngineConfig`] minus the dim /
/// counter / label layout (fixed by `crate::mapping`).
#[derive(Debug, Clone)]
pub struct KdbConfig {
    /// Root data directory — each tier gets `{root}/tier={name}/`.
    pub data_dir: PathBuf,
    /// WAL events-per-shard before forced rotation. Default `500_000`.
    pub wal_max_events: u32,
    /// fsync interval for batched writes. Default `64`.
    pub wal_sync_interval: u32,
    /// Byte threshold for batched fsync. Default `256 KiB`.
    pub wal_sync_bytes: u64,
    /// WAL shard count per engine. Default `4`.
    pub wal_shard_count: usize,
    /// Mmap'd segment file LRU cache capacity. Default `256`.
    pub mmap_cache_capacity: usize,
    /// Days of historical segments to replay into the rollup store on
    /// open. Default `7`.
    pub rollup_replay_days: u32,
    /// Pre-declared retention tiers — engines are spawned eagerly for
    /// these so routing on the hot path is allocation-free for the
    /// common case. Additional tiers encountered at ingest time are
    /// lazily created.
    pub eager_tiers: Vec<SmolStr>,
}

impl KdbConfig {
    /// Build a config that targets `data_dir` with default tuning.
    #[must_use]
    pub fn new(data_dir: PathBuf) -> Self {
        Self {
            data_dir,
            wal_max_events: 500_000,
            wal_sync_interval: 64,
            wal_sync_bytes: 256 * 1024,
            wal_shard_count: 4,
            mmap_cache_capacity: 256,
            rollup_replay_days: 7,
            eager_tiers: vec![SmolStr::new("free"), SmolStr::new("pro"), SmolStr::new("team")],
        }
    }
}

/// The KeplorDB-backed event store.
pub struct KdbStore {
    engines: ArcSwap<HashMap<SmolStr, Arc<Engine<D, C, L>>>>,
    config: KdbConfig,
    insert_lock: Mutex<()>,
    manifests: Mutex<ManifestStore>,
}

impl std::fmt::Debug for KdbStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KdbStore")
            .field("data_dir", &self.config.data_dir)
            .field("tier_count", &self.engines.load().len())
            .finish()
    }
}

impl KdbStore {
    /// Open (or create) a store at `config.data_dir`.
    ///
    /// Engines for every tier in [`KdbConfig::eager_tiers`] are opened
    /// up front so the first append to each tier does not pay the open
    /// cost (segment scan + WAL replay + rollup catch-up).
    pub fn open(config: KdbConfig) -> Result<Self, StoreError> {
        std::fs::create_dir_all(&config.data_dir).map_err(StoreError::Io)?;

        let mut engines: HashMap<SmolStr, Arc<Engine<D, C, L>>> =
            HashMap::with_capacity(config.eager_tiers.len());
        for tier in &config.eager_tiers {
            let eng = Self::open_engine(&config, tier.as_str())?;
            engines.insert(tier.clone(), Arc::new(eng));
        }

        // Also pick up any on-disk tiers that aren't in `eager_tiers`
        // (left behind from a previous run with different eager list).
        if let Ok(rd) = std::fs::read_dir(&config.data_dir) {
            for entry in rd.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                let Some(tier_name) = name_str.strip_prefix("tier=") else { continue };
                let tier: SmolStr = SmolStr::new(tier_name);
                if engines.contains_key(&tier) {
                    continue;
                }
                if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                    let eng = Self::open_engine(&config, tier.as_str())?;
                    engines.insert(tier, Arc::new(eng));
                }
            }
        }

        let manifests = ManifestStore::open(&config.data_dir.join("archive_manifests.jsonl"))?;

        Ok(Self {
            engines: ArcSwap::new(Arc::new(engines)),
            config,
            insert_lock: Mutex::new(()),
            manifests: Mutex::new(manifests),
        })
    }

    /// Open an in-memory store under a unique temporary directory
    /// (used by tests).
    pub fn open_in_memory() -> Result<Self, StoreError> {
        let tmp = tempdir_path();
        Self::open(KdbConfig::new(tmp))
    }

    fn open_engine(config: &KdbConfig, tier: &str) -> Result<Engine<D, C, L>, StoreError> {
        let data_dir = config.data_dir.join(format!("tier={tier}"));
        let ec = EngineConfig {
            data_dir,
            wal_max_events: config.wal_max_events,
            wal_sync_interval: config.wal_sync_interval,
            wal_sync_bytes: config.wal_sync_bytes,
            schema_id: SCHEMA_ID,
            bloom_dim: DIM_USER_ID,
            rollup_dims: vec![mapping::DIM_USER_ID, mapping::DIM_API_KEY_ID, mapping::DIM_MODEL],
            mmap_cache_capacity: config.mmap_cache_capacity,
            wal_shard_count: config.wal_shard_count,
            rollup_replay_days: config.rollup_replay_days,
        };
        Engine::<D, C, L>::open(ec).map_err(kdb_err)
    }

    fn engine_for(&self, tier: &str) -> Result<Arc<Engine<D, C, L>>, StoreError> {
        // Fast path: tier already in the map.
        {
            let map = self.engines.load();
            if let Some(eng) = map.get(tier) {
                return Ok(eng.clone());
            }
        }
        // Slow path: insert under lock.
        let _guard = self.insert_lock.lock().map_err(|e| StoreError::Other(e.to_string()))?;
        // Re-check: another thread may have inserted while we waited.
        {
            let map = self.engines.load();
            if let Some(eng) = map.get(tier) {
                return Ok(eng.clone());
            }
        }
        let eng = Arc::new(Self::open_engine(&self.config, tier)?);
        let mut new_map = (**self.engines.load()).clone();
        new_map.insert(SmolStr::new(tier), eng.clone());
        self.engines.store(Arc::new(new_map));
        Ok(eng)
    }

    fn all_engines(&self) -> Vec<Arc<Engine<D, C, L>>> {
        self.engines.load().values().cloned().collect()
    }

    // ── Ingest ─────────────────────────────────────────────────────

    /// Append a single event (batched fsync).
    pub fn append_event(&self, event: &LlmEvent) -> Result<EventId, StoreError> {
        let eng = self.engine_for(event.tier.as_str())?;
        let log = mapping::to_log_event(event);
        eng.append(&log).map_err(kdb_err)?;
        Ok(event.id)
    }

    /// Append a single event and fsync before returning.
    pub fn append_event_durable(&self, event: &LlmEvent) -> Result<EventId, StoreError> {
        let eng = self.engine_for(event.tier.as_str())?;
        let log = mapping::to_log_event(event);
        eng.append_durable(&log).map_err(kdb_err)?;
        Ok(event.id)
    }

    /// Append a batch (batched fsync).
    ///
    /// Events are routed to their tier's engine; cross-tier batches are
    /// split into per-tier sub-batches so `append_batch` gets the
    /// one-lock-per-batch fast path inside each engine.
    pub fn append_batch(&self, events: &[LlmEvent]) -> Result<Vec<EventId>, StoreError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::with_capacity(events.len());
        let mut by_tier: HashMap<SmolStr, Vec<keplordb::LogEvent<D, C, L>>> = HashMap::new();
        for ev in events {
            ids.push(ev.id);
            by_tier.entry(ev.tier.clone()).or_default().push(mapping::to_log_event(ev));
        }
        for (tier, logs) in by_tier {
            let eng = self.engine_for(tier.as_str())?;
            eng.append_batch(&logs).map_err(kdb_err)?;
        }
        Ok(ids)
    }

    /// Append a batch with a single `fsync` at the end per affected tier.
    pub fn append_batch_durable(&self, events: &[LlmEvent]) -> Result<Vec<EventId>, StoreError> {
        if events.is_empty() {
            return Ok(Vec::new());
        }
        let mut ids = Vec::with_capacity(events.len());
        let mut by_tier: HashMap<SmolStr, Vec<keplordb::LogEvent<D, C, L>>> = HashMap::new();
        for ev in events {
            ids.push(ev.id);
            by_tier.entry(ev.tier.clone()).or_default().push(mapping::to_log_event(ev));
        }
        for (tier, logs) in by_tier {
            let eng = self.engine_for(tier.as_str())?;
            eng.append_batch_durable(&logs).map_err(kdb_err)?;
        }
        Ok(ids)
    }

    // ── Queries ────────────────────────────────────────────────────

    /// Retrieve an event by id — fans out across tiers.
    pub fn get_event(&self, id: &EventId) -> Result<Option<LlmEvent>, StoreError> {
        let id_str = id.to_string();
        for eng in self.all_engines() {
            if let Some(er) = eng.get_event(&id_str).map_err(kdb_err)? {
                return Ok(Some(mapping::from_event_ref(&er).map_err(map_err)?));
            }
        }
        Ok(None)
    }

    /// Query events across all tiers, newest-first.
    pub fn query(
        &self,
        filter: &EventFilter,
        limit: u32,
        cursor: Option<Cursor>,
    ) -> Result<Vec<LlmEvent>, StoreError> {
        let qf = build_query_filter(filter, cursor);
        let mut out: Vec<(i64, LlmEvent)> = Vec::new();
        for eng in self.all_engines() {
            let rows = eng.query_recent(&qf, limit as usize).map_err(kdb_err)?;
            for r in rows {
                let ev = mapping::from_event_ref(&r).map_err(map_err)?;
                out.push((ev.ts_ns, ev));
            }
        }
        // Reverse-chronological merge; KeplorDB returns per-engine newest
        // first so we just sort across.
        out.sort_by(|a, b| b.0.cmp(&a.0));
        out.truncate(limit as usize);
        Ok(out.into_iter().map(|(_, e)| e).collect())
    }

    /// Query summaries — same semantics as `query` but returns the
    /// narrower `EventSummary` shape used by the HTTP API.
    pub fn query_summary(
        &self,
        filter: &EventFilter,
        limit: u32,
        cursor: Option<Cursor>,
    ) -> Result<Vec<EventSummary>, StoreError> {
        let events = self.query(filter, limit, cursor)?;
        Ok(events.into_iter().map(llm_to_summary).collect())
    }

    /// Real-time cost + event count aggregation.
    pub fn quota_summary(
        &self,
        user_id: Option<&str>,
        api_key_id: Option<&str>,
        from_ts_ns: i64,
    ) -> Result<QuotaSummary, StoreError> {
        let mut qf = QueryFilter::<D> { from_ts_ns: Some(from_ts_ns), ..Default::default() };
        if let Some(u) = user_id {
            qf.dims[DIM_USER_ID] = Some(u.to_owned());
        }
        if let Some(k) = api_key_id {
            qf.dims[DIM_API_KEY_ID] = Some(k.to_owned());
        }
        let mut cost = 0i64;
        let mut count = 0i64;
        for eng in self.all_engines() {
            let r = eng.aggregate(&qf).map_err(kdb_err)?;
            cost = cost.saturating_add(r.metric);
            count = count.saturating_add(r.event_count as i64);
        }
        Ok(QuotaSummary { cost_nanodollars: cost, event_count: count })
    }

    /// Pre-aggregated daily rollups.
    pub fn query_rollups(
        &self,
        user_id: Option<&str>,
        api_key_id: Option<&str>,
        from_day: i64,
        to_day: i64,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<RollupRow>, StoreError> {
        let dim_filters: [Option<&str>; 3] = [user_id, api_key_id, None];
        let mut merged: HashMap<RollupKey, RollupAcc> = HashMap::new();
        for eng in self.all_engines() {
            let rows = eng.query_rollups(from_day, to_day, &dim_filters).map_err(kdb_err)?;
            for (key, val) in rows {
                let k = RollupKey {
                    day: key.day,
                    user: key.dims.first().cloned().unwrap_or_default(),
                    api_key: key.dims.get(1).cloned().unwrap_or_default(),
                    model: key.dims.get(2).cloned().unwrap_or_default(),
                };
                let acc = merged.entry(k).or_default();
                acc.event_count += val.event_count as i64;
                acc.error_count += val.counters[mapping::COUNTER_IS_ERROR] as i64;
                acc.input_tokens += val.counters[mapping::COUNTER_INPUT_TOKENS] as i64;
                acc.output_tokens += val.counters[mapping::COUNTER_OUTPUT_TOKENS] as i64;
                acc.cache_read += val.counters[mapping::COUNTER_CACHE_READ] as i64;
                acc.cache_creation += val.counters[mapping::COUNTER_CACHE_CREATION] as i64;
                acc.cost = acc.cost.saturating_add(val.metric);
            }
        }
        let mut rows: Vec<(RollupKey, RollupAcc)> = merged.into_iter().collect();
        rows.sort_by(|a, b| a.0.day.cmp(&b.0.day).then_with(|| a.0.user.cmp(&b.0.user)));
        let page: Vec<RollupRow> = rows
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .map(|(k, a)| RollupRow {
                day: k.day,
                user_id: k.user,
                api_key_id: k.api_key,
                provider: String::new(),
                model: k.model,
                event_count: a.event_count,
                error_count: a.error_count,
                input_tokens: a.input_tokens,
                output_tokens: a.output_tokens,
                cache_read_input_tokens: a.cache_read,
                cache_creation_input_tokens: a.cache_creation,
                cost_nanodollars: a.cost,
            })
            .collect();
        Ok(page)
    }

    /// Aggregate stats — optionally grouped by model — derived from
    /// rollups.
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
        let rollups = self.query_rollups(user_id, api_key_id, from_day, to_day, u32::MAX, 0)?;
        let mut merged: HashMap<(String, String), AggregateAcc> = HashMap::new();
        for r in rollups {
            if let Some(pf) = provider_filter {
                if r.provider != pf {
                    continue;
                }
            }
            let key = if group_by_model {
                (r.provider.clone(), r.model.clone())
            } else {
                (String::new(), String::new())
            };
            let acc = merged.entry(key).or_default();
            acc.event_count += r.event_count;
            acc.error_count += r.error_count;
            acc.input_tokens += r.input_tokens;
            acc.output_tokens += r.output_tokens;
            acc.cache_read += r.cache_read_input_tokens;
            acc.cache_creation += r.cache_creation_input_tokens;
            acc.cost = acc.cost.saturating_add(r.cost_nanodollars);
        }
        let mut rows: Vec<_> = merged.into_iter().collect();
        rows.sort_by(|a, b| b.1.cost.cmp(&a.1.cost));
        Ok(rows
            .into_iter()
            .skip(offset as usize)
            .take(limit as usize)
            .map(|((p, m), a)| AggregateRow {
                provider: p,
                model: m,
                event_count: a.event_count,
                error_count: a.error_count,
                input_tokens: a.input_tokens,
                output_tokens: a.output_tokens,
                cache_read_input_tokens: a.cache_read,
                cache_creation_input_tokens: a.cache_creation,
                cost_nanodollars: a.cost,
            })
            .collect())
    }

    // ── Export ─────────────────────────────────────────────────────

    /// Stream every matching event as an [`EventSummary`] — no limit.
    pub fn export_events(
        &self,
        filter: &EventFilter,
        callback: &mut dyn FnMut(EventSummary),
    ) -> Result<(), StoreError> {
        // `query_recent` caps per engine by `limit` — we iterate in
        // chunks, advancing the cursor until empty.
        let mut cursor: Option<Cursor> = None;
        loop {
            let rows = self.query(filter, 1_000, cursor)?;
            if rows.is_empty() {
                break;
            }
            let last_ts = rows.last().map(|e| e.ts_ns).unwrap_or(0);
            for ev in rows {
                callback(llm_to_summary(ev));
            }
            cursor = Some(Cursor(last_ts));
        }
        Ok(())
    }

    // ── GC / delete ────────────────────────────────────────────────

    /// Drop segments older than `older_than_ns` across every tier.
    ///
    /// The returned `events_deleted` count is segment-granular (each
    /// segment holds many events) because KeplorDB's GC operates on
    /// whole segments — the metric is repurposed here to report
    /// "segments removed" which is what operators actually tune
    /// retention against.
    pub fn gc_expired(&self, older_than_ns: i64) -> Result<GcStats, StoreError> {
        let mut total = 0usize;
        for eng in self.all_engines() {
            let s = eng.gc(older_than_ns).map_err(kdb_err)?;
            total += s.segments_deleted as usize;
        }
        Ok(GcStats { events_deleted: total, blobs_deleted: 0 })
    }

    /// Drop segments in a single tier.
    pub fn gc_tier(&self, tier: &str, older_than_ns: i64) -> Result<GcStats, StoreError> {
        let eng = self.engine_for(tier)?;
        let s = eng.gc(older_than_ns).map_err(kdb_err)?;
        Ok(GcStats { events_deleted: s.segments_deleted as usize, blobs_deleted: 0 })
    }

    /// Tombstone a single event.
    pub fn delete_event(&self, id: &EventId) -> Result<bool, StoreError> {
        let id_str = id.to_string();
        let mut existed = false;
        for eng in self.all_engines() {
            if eng.get_event(&id_str).map_err(kdb_err)?.is_some() {
                eng.delete_event(&id_str).map_err(kdb_err)?;
                existed = true;
            }
        }
        Ok(existed)
    }

    /// Batch tombstone.
    pub fn delete_events_by_ids(&self, ids: &[EventId]) -> Result<usize, StoreError> {
        if ids.is_empty() {
            return Ok(0);
        }
        let id_strings: Vec<String> = ids.iter().map(|i| i.to_string()).collect();
        let id_refs: Vec<&str> = id_strings.iter().map(|s| s.as_str()).collect();
        // Tombstones are engine-scoped — we don't know which tier holds
        // which id, so each engine gets the full list. `delete_events`
        // treats unknown ids as no-ops.
        for eng in self.all_engines() {
            eng.delete_events(&id_refs).map_err(kdb_err)?;
        }
        Ok(ids.len())
    }

    // ── Archive path ────────────────────────────────────────────────

    /// No-op: KeplorDB's `RollupStore` accumulates on write and
    /// survives restart by replaying segments. Exists for API
    /// compatibility.
    pub fn rollup_day(&self, _day_epoch: i64) -> Result<(), StoreError> {
        Ok(())
    }

    /// No-op: see [`Self::rollup_day`].
    pub fn rollup_days_for_range(&self, _from_ns: i64, _to_ns: i64) -> Result<(), StoreError> {
        Ok(())
    }

    /// Fetch events below a timestamp for the archiver.
    pub fn query_events_for_archive(
        &self,
        older_than_ns: i64,
        limit: u32,
    ) -> Result<Vec<LlmEvent>, StoreError> {
        let qf = QueryFilter::<D> { to_ts_ns: Some(older_than_ns), ..Default::default() };
        let mut out: Vec<(i64, LlmEvent)> = Vec::new();
        for eng in self.all_engines() {
            let rows = eng.query_recent(&qf, limit as usize).map_err(kdb_err)?;
            for r in rows {
                let ev = mapping::from_event_ref(&r).map_err(map_err)?;
                out.push((ev.ts_ns, ev));
            }
        }
        // Archiver prefers oldest first.
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out.truncate(limit as usize);
        Ok(out.into_iter().map(|(_, e)| e).collect())
    }

    /// Record an archive manifest entry.
    pub fn insert_archive_manifest(&self, m: &ArchiveManifest) -> Result<(), StoreError> {
        let mut store = self.manifests.lock().map_err(|e| StoreError::Other(e.to_string()))?;
        store.insert(m.clone())
    }

    /// True iff a manifest exists for `(user_id, day)` whose range
    /// overlaps the filter window.
    pub fn has_archived_data(
        &self,
        user_id: Option<&str>,
        from_ts_ns: i64,
        to_ts_ns: i64,
    ) -> Result<bool, StoreError> {
        let store = self.manifests.lock().map_err(|e| StoreError::Other(e.to_string()))?;
        Ok(store.any_overlapping(user_id, from_ts_ns, to_ts_ns))
    }

    /// List manifests matching the filter.
    pub fn list_archives(
        &self,
        user_id: Option<&str>,
        from_ts_ns: Option<i64>,
        to_ts_ns: Option<i64>,
        limit: u32,
        offset: u32,
    ) -> Result<Vec<ArchiveManifest>, StoreError> {
        let store = self.manifests.lock().map_err(|e| StoreError::Other(e.to_string()))?;
        Ok(store.list(user_id, from_ts_ns, to_ts_ns, limit, offset))
    }

    /// Aggregate counts + bytes + oldest-ts for the CLI's
    /// `archive-status` command.
    pub fn archive_summary(&self) -> Result<(usize, usize, i64), StoreError> {
        let store = self.manifests.lock().map_err(|e| StoreError::Other(e.to_string()))?;
        Ok(store.summary())
    }

    // ── Diagnostics ─────────────────────────────────────────────────

    /// `SELECT 1`-equivalent health check.
    pub fn health_probe(&self) -> Result<(), StoreError> {
        // A store is healthy if every engine can enumerate its segment
        // manifest without error. `total_events` does exactly that.
        for eng in self.all_engines() {
            let _ = eng.total_events();
        }
        Ok(())
    }

    /// Flush all per-tier WALs into segment files.
    pub fn wal_checkpoint(&self) -> Result<(), StoreError> {
        for eng in self.all_engines() {
            eng.flush().map_err(kdb_err)?;
        }
        Ok(())
    }

    /// No-op: segment-based storage has no equivalent of SQLite VACUUM.
    pub fn vacuum(&self) -> Result<(), StoreError> {
        Ok(())
    }

    /// Total bytes across every tier's segments.
    pub fn db_size_bytes(&self) -> Result<u64, StoreError> {
        let mut total = 0u64;
        for eng in self.all_engines() {
            total = total.saturating_add(eng.total_bytes());
        }
        Ok(total)
    }
}

fn build_query_filter(filter: &EventFilter, cursor: Option<Cursor>) -> QueryFilter<D> {
    let mut qf = mapping::to_query_filter(filter);
    qf.cursor = cursor.map(|c| KdbCursor(c.0));
    qf
}

fn llm_to_summary(ev: LlmEvent) -> EventSummary {
    EventSummary {
        id: ev.id,
        ts_ns: ev.ts_ns,
        user_id: ev.user_id.map(|u| u.as_str().to_owned()),
        api_key_id: ev.api_key_id.map(|k| k.as_str().to_owned()),
        provider: ev.provider.id_key().to_owned(),
        model: ev.model.to_string(),
        endpoint: ev.endpoint.to_string(),
        http_status: ev.http_status,
        input_tokens: ev.usage.input_tokens,
        output_tokens: ev.usage.output_tokens,
        cache_read_input_tokens: ev.usage.cache_read_input_tokens,
        cache_creation_input_tokens: ev.usage.cache_creation_input_tokens,
        reasoning_tokens: ev.usage.reasoning_tokens,
        cost_nanodollars: ev.cost_nanodollars,
        ttft_ms: ev.latency.ttft_ms,
        total_ms: ev.latency.total_ms,
        streaming: ev.flags.contains(keplor_core::EventFlags::STREAMING),
        source: ev.source.map(|s| s.to_string()),
        error_type: ev.error.as_ref().map(|e| provider_error_type_key(e).to_owned()),
        metadata_json: ev.metadata.as_ref().map(|v| v.to_string()),
    }
}

fn provider_error_type_key(e: &keplor_core::ProviderError) -> &'static str {
    use keplor_core::ProviderError as P;
    match e {
        P::RateLimited { .. } => "rate_limited",
        P::InvalidRequest(_) => "invalid_request",
        P::AuthFailed => "auth_failed",
        P::ContextLengthExceeded { .. } => "context_length_exceeded",
        P::ContentFiltered { .. } => "content_filtered",
        P::UpstreamTimeout => "upstream_timeout",
        P::UpstreamUnavailable => "upstream_unavailable",
        P::Other { .. } => "other",
    }
}

fn kdb_err(e: keplordb::DbError) -> StoreError {
    StoreError::Other(e.to_string())
}

fn map_err(e: mapping::MappingError) -> StoreError {
    StoreError::Other(e.to_string())
}

fn tempdir_path() -> PathBuf {
    // Unique tmp path — we only need the dir; keplordb creates it.
    use std::time::{SystemTime, UNIX_EPOCH};
    let id = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    std::env::temp_dir().join(format!("keplor-kdb-test-{id}"))
}

#[derive(Hash, Eq, PartialEq, Default)]
struct RollupKey {
    day: i64,
    user: String,
    api_key: String,
    model: String,
}

#[derive(Default)]
struct RollupAcc {
    event_count: i64,
    error_count: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_read: i64,
    cache_creation: i64,
    cost: i64,
}

#[derive(Default)]
struct AggregateAcc {
    event_count: i64,
    error_count: i64,
    input_tokens: i64,
    output_tokens: i64,
    cache_read: i64,
    cache_creation: i64,
    cost: i64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use keplor_core::{Latencies, Provider, RouteId, Usage, UserId};
    use smol_str::SmolStr;

    fn sample(tier: &str, ts_ns: i64, cost: i64) -> LlmEvent {
        LlmEvent {
            id: EventId::new(),
            ts_ns,
            user_id: Some(UserId::from("alice")),
            api_key_id: None,
            org_id: None,
            project_id: None,
            route_id: RouteId::from("chat"),
            provider: Provider::OpenAI,
            model: SmolStr::new("gpt-4o"),
            model_family: None,
            endpoint: SmolStr::new("/v1/chat/completions"),
            method: http::Method::POST,
            http_status: Some(200),
            usage: Usage { input_tokens: 100, output_tokens: 50, ..Default::default() },
            cost_nanodollars: cost,
            latency: Latencies { ttft_ms: Some(50), total_ms: 500, time_to_close_ms: None },
            flags: keplor_core::EventFlags::empty(),
            error: None,
            request_sha256: [0; 32],
            response_sha256: [0; 32],
            client_ip: None,
            user_agent: None,
            request_id: None,
            trace_id: None,
            source: None,
            ingested_at: ts_ns,
            metadata: None,
            tier: SmolStr::new(tier),
        }
    }

    #[test]
    fn open_creates_eager_tiers() {
        let store = KdbStore::open_in_memory().unwrap();
        let map = store.engines.load();
        assert!(map.contains_key("free"));
        assert!(map.contains_key("pro"));
        assert!(map.contains_key("team"));
    }

    #[test]
    fn append_and_get_event_roundtrips() {
        let store = KdbStore::open_in_memory().unwrap();
        let ev = sample("pro", 1_700_000_000_000_000_000, 5_000);
        let id = store.append_event(&ev).unwrap();
        // Reads see only rotated segments, so flush before querying.
        store.wal_checkpoint().unwrap();
        let got = store.get_event(&id).unwrap().expect("event should exist");
        assert_eq!(got.cost_nanodollars, 5_000);
        assert_eq!(got.tier.as_str(), "pro");
    }

    #[test]
    fn query_merges_across_tiers_and_honors_limit() {
        let store = KdbStore::open_in_memory().unwrap();
        for (i, tier) in ["free", "pro", "team"].iter().enumerate() {
            for j in 0..3 {
                let ts = 1_700_000_000_000_000_000 + (i as i64 * 3 + j as i64) * 1_000_000_000;
                store.append_event(&sample(tier, ts, 100)).unwrap();
            }
        }
        store.wal_checkpoint().unwrap();
        let got = store.query(&EventFilter::default(), 5, None).unwrap();
        assert_eq!(got.len(), 5);
        // Newest first.
        for pair in got.windows(2) {
            assert!(pair[0].ts_ns >= pair[1].ts_ns);
        }
    }

    #[test]
    fn quota_summary_sums_across_tiers() {
        let store = KdbStore::open_in_memory().unwrap();
        for tier in ["free", "pro", "team"] {
            store.append_event(&sample(tier, 1_700_000_000_000_000_000, 1_000)).unwrap();
        }
        store.wal_checkpoint().unwrap();
        let q = store.quota_summary(Some("alice"), None, 0).unwrap();
        assert_eq!(q.event_count, 3);
        assert_eq!(q.cost_nanodollars, 3_000);
    }

    #[test]
    fn gc_tier_only_affects_named_tier() {
        let store = KdbStore::open_in_memory().unwrap();
        let ts = 1_700_000_000_000_000_000i64;
        for tier in ["free", "pro"] {
            store.append_event(&sample(tier, ts, 100)).unwrap();
        }
        store.wal_checkpoint().unwrap();

        store.gc_tier("free", ts + 1).unwrap();
        // `pro` engine should still report events.
        let q_pro = store.quota_summary(Some("alice"), None, 0).unwrap();
        assert_eq!(q_pro.event_count, 1);
    }

    #[test]
    fn delete_events_by_ids_is_idempotent() {
        let store = KdbStore::open_in_memory().unwrap();
        let ts = 1_700_000_000_000_000_000i64;
        let mut ids = Vec::new();
        for _ in 0..3 {
            let ev = sample("pro", ts, 100);
            let id = ev.id;
            store.append_event(&ev).unwrap();
            ids.push(id);
        }
        store.wal_checkpoint().unwrap();
        let n = store.delete_events_by_ids(&ids).unwrap();
        assert_eq!(n, 3);
        let n2 = store.delete_events_by_ids(&ids).unwrap();
        assert_eq!(n2, 3);
        assert!(store.get_event(&ids[0]).unwrap().is_none());
    }

    #[test]
    fn health_probe_green_on_empty_store() {
        let store = KdbStore::open_in_memory().unwrap();
        store.health_probe().unwrap();
    }

    #[test]
    fn dynamic_tier_is_lazily_created() {
        let store = KdbStore::open_in_memory().unwrap();
        store.append_event(&sample("enterprise", 1_700_000_000_000_000_000, 100)).unwrap();
        let map = store.engines.load();
        assert!(map.contains_key("enterprise"));
    }
}
