//! Event archival to S3/R2 as zstd-compressed JSONL files.
//!
//! When the SQLite database grows past a configured threshold (or events
//! age past a deadline), the [`Archiver`] serializes old events into
//! JSONL files, compresses them with zstd, uploads to an S3-compatible
//! store, records a manifest row in SQLite, and deletes the archived
//! events from `llm_events`.
//!
//! ## S3 key layout (Hive-style)
//!
//! ```text
//! {prefix}/user_id={user_id}/day={YYYY-MM-DD}/{archive_id}.jsonl.zstd
//! {prefix}/user_id=_none/day={YYYY-MM-DD}/{archive_id}.jsonl.zstd
//! ```

use std::collections::BTreeMap;
use std::sync::Arc;

use keplor_core::LlmEvent;
use object_store::path::Path as ObjPath;
use object_store::{ObjectStore, ObjectStoreExt, PutPayload};

use crate::error::StoreError;
use crate::kdb_store::KdbStore;
use crate::stored_event::StoredEvent;
use crate::types::ArchiveManifest;

/// S3 connection configuration for the archiver.
#[derive(Debug, Clone)]
pub struct ArchiveS3Config {
    /// S3 bucket name.
    pub bucket: String,
    /// S3 endpoint URL.
    pub endpoint: String,
    /// S3 region.
    pub region: String,
    /// Access key ID.
    pub access_key_id: String,
    /// Secret access key.
    pub secret_access_key: String,
    /// Key prefix in the bucket.
    pub prefix: String,
    /// Use path-style addressing (for MinIO).
    pub path_style: bool,
}

/// Archives old events from SQLite to S3/R2.
pub struct Archiver {
    store: Arc<KdbStore>,
    client: Box<dyn ObjectStore>,
    prefix: String,
}

impl Archiver {
    /// Create a new archiver.
    pub fn new(
        store: Arc<KdbStore>,
        config: &ArchiveS3Config,
        rt: tokio::runtime::Handle,
    ) -> Result<Self, StoreError> {
        let client = build_s3_client(config, rt)?;
        Ok(Self { store, client, prefix: config.prefix.clone() })
    }

    /// Validate S3 connectivity with a probe HEAD request.
    ///
    /// Call at startup to fail fast on invalid credentials or
    /// unreachable endpoints instead of discovering errors hours
    /// later on the first archive cycle.
    pub fn probe(&self) -> Result<(), StoreError> {
        let probe_path = ObjPath::from(format!("{}/_probe", self.prefix));
        // A HEAD on a non-existent key returns 404, not an auth error.
        // An auth failure returns 403. Both are valid S3 responses.
        match tokio::runtime::Handle::current().block_on(self.client.head(&probe_path)) {
            Ok(_) => Ok(()),
            Err(object_store::Error::NotFound { .. }) => Ok(()), // 404 = reachable
            Err(e) => Err(StoreError::ArchiveS3(format!("probe failed: {e}"))),
        }
    }

    /// Archive events older than `older_than_ns`.
    ///
    /// 1. Force rollup for affected days (so daily_rollups survive deletion).
    /// 2. Query events grouped by (user_id, day).
    /// 3. For each chunk: serialize → compress → upload → manifest → delete.
    /// 4. VACUUM if events were deleted.
    ///
    /// **Per-chunk error isolation**: if an S3 upload fails, that chunk is
    /// skipped and its events remain in SQLite.  The next archive cycle
    /// will retry them.  Events are **never** deleted unless the upload
    /// is confirmed.
    pub fn archive_old_events(
        &self,
        older_than_ns: i64,
        batch_size: usize,
    ) -> Result<ArchiveResult, StoreError> {
        // Step 1: force rollup for all days in the range.
        self.store.rollup_days_for_range(0, older_than_ns)?;

        // Step 2: fetch events ordered for grouping.
        let events = self.store.query_events_for_archive(older_than_ns)?;
        if events.is_empty() {
            return Ok(ArchiveResult::default());
        }

        // Group by (user_id, day).
        let groups = group_events(events);

        let mut total_archived = 0usize;
        let mut total_files = 0usize;
        let mut total_bytes = 0usize;
        let mut total_failed = 0usize;

        for ((user_key, day), events) in &groups {
            for chunk in events.chunks(batch_size) {
                match self.archive_chunk(chunk, user_key, day) {
                    Ok((compressed_bytes, ids)) => {
                        // Upload confirmed — safe to delete from SQLite.
                        if let Err(e) = self.store.delete_events_by_ids(&ids) {
                            tracing::error!(
                                user = user_key,
                                day,
                                events = ids.len(),
                                error = %e,
                                "failed to delete archived events from SQLite \
                                 — events are duplicated in S3 and SQLite, \
                                 will be cleaned up on next GC cycle"
                            );
                            continue;
                        }
                        total_archived += ids.len();
                        total_files += 1;
                        total_bytes += compressed_bytes;
                    },
                    Err(e) => {
                        // S3 upload failed — skip this chunk.  Events
                        // remain in SQLite and will be retried next cycle.
                        tracing::warn!(
                            user = user_key,
                            day,
                            events = chunk.len(),
                            error = %e,
                            "archive chunk failed — events stay in SQLite, \
                             will retry on next cycle"
                        );
                        total_failed += chunk.len();
                        continue;
                    },
                }
            }
        }

        // VACUUM to reclaim space.
        if total_archived > 0 {
            if let Err(e) = self.store.vacuum() {
                tracing::warn!(error = %e, "vacuum after archive failed");
            }
        }

        if total_failed > 0 {
            tracing::warn!(
                failed = total_failed,
                archived = total_archived,
                "archive cycle completed with failures — failed events will retry"
            );
        }

        Ok(ArchiveResult {
            events_archived: total_archived,
            files_uploaded: total_files,
            compressed_bytes: total_bytes,
            events_failed: total_failed,
        })
    }

    /// Archive a single chunk: serialize → compress → upload → manifest.
    ///
    /// Returns `(compressed_bytes, event_ids)` on success.
    /// On failure, returns the error — caller decides whether to skip or abort.
    fn archive_chunk(
        &self,
        chunk: &[LlmEvent],
        user_key: &str,
        day: &str,
    ) -> Result<(usize, Vec<keplor_core::EventId>), StoreError> {
        let archive_id = ulid::Ulid::new().to_string();

        // Serialize to JSONL.
        let mut jsonl = Vec::with_capacity(chunk.len() * 512);
        let mut min_ts = i64::MAX;
        let mut max_ts = i64::MIN;
        let mut ids = Vec::with_capacity(chunk.len());

        for event in chunk {
            let stored = StoredEvent::from(event);
            serde_json::to_writer(&mut jsonl, &stored)
                .map_err(|e| StoreError::Internal(format!("json serialize: {e}")))?;
            jsonl.push(b'\n');
            min_ts = min_ts.min(event.ts_ns);
            max_ts = max_ts.max(event.ts_ns);
            ids.push(event.id);
        }

        // Compress with zstd.
        let compressed = zstd::bulk::compress(&jsonl, 3)
            .map_err(|e| StoreError::Internal(format!("zstd compress: {e}")))?;

        // Build S3 key.
        let s3_key = format!(
            "{prefix}/user_id={user}/day={day}/{id}.jsonl.zstd",
            prefix = self.prefix,
            user = user_key,
            id = archive_id,
        );
        let obj_path = ObjPath::from(s3_key.clone());

        // Upload to S3 — this is the critical step.
        let payload = PutPayload::from(compressed.clone());
        tokio::runtime::Handle::current()
            .block_on(self.client.put(&obj_path, payload))
            .map_err(|e| StoreError::ArchiveS3(format!("put {s3_key}: {e}")))?;

        // Upload confirmed — record manifest.
        let manifest = ArchiveManifest {
            archive_id: archive_id.clone(),
            user_id: user_key.to_owned(),
            day: day.to_owned(),
            s3_key: s3_key.clone(),
            event_count: chunk.len(),
            min_ts_ns: min_ts,
            max_ts_ns: max_ts,
            compressed_bytes: compressed.len(),
            created_at: now_epoch_secs(),
        };
        self.store.insert_archive_manifest(&manifest)?;

        tracing::info!(
            archive_id,
            user = user_key,
            day,
            events = chunk.len(),
            compressed_bytes = compressed.len(),
            "archived event chunk to S3"
        );

        Ok((compressed.len(), ids))
    }

    /// Fetch archived events from S3 for a given user and day range.
    pub fn fetch_archived_events(
        &self,
        manifests: &[ArchiveManifest],
    ) -> Result<Vec<LlmEvent>, StoreError> {
        let mut all_events = Vec::new();

        for manifest in manifests {
            let obj_path = ObjPath::from(manifest.s3_key.clone());

            let result = tokio::runtime::Handle::current()
                .block_on(self.client.get(&obj_path))
                .map_err(|e| StoreError::ArchiveS3(format!("get {}: {e}", manifest.s3_key)))?;

            let compressed = tokio::runtime::Handle::current()
                .block_on(result.bytes())
                .map_err(|e| StoreError::ArchiveS3(format!("read {}: {e}", manifest.s3_key)))?;

            let decompressed = zstd::bulk::decompress(&compressed, 100 * 1024 * 1024)
                .map_err(|e| StoreError::Internal(format!("zstd decompress: {e}")))?;

            let text = std::str::from_utf8(&decompressed)
                .map_err(|e| StoreError::Internal(format!("invalid utf8: {e}")))?;

            for line in text.lines() {
                if line.is_empty() {
                    continue;
                }
                let stored: StoredEvent = serde_json::from_str(line)
                    .map_err(|e| StoreError::Internal(format!("json parse: {e}")))?;
                let event: LlmEvent = stored.try_into()?;
                all_events.push(event);
            }
        }

        Ok(all_events)
    }
}

/// Result of an archive operation.
#[derive(Debug, Default)]
pub struct ArchiveResult {
    /// Total events moved from SQLite to S3.
    pub events_archived: usize,
    /// Number of JSONL files uploaded.
    pub files_uploaded: usize,
    /// Total compressed bytes uploaded.
    pub compressed_bytes: usize,
    /// Events that failed to archive (remain in SQLite for retry).
    pub events_failed: usize,
}

/// Group events by (user_id, day) for batched archival.
fn group_events(events: Vec<LlmEvent>) -> BTreeMap<(String, String), Vec<LlmEvent>> {
    let mut groups: BTreeMap<(String, String), Vec<LlmEvent>> = BTreeMap::new();

    for event in events {
        let user_key =
            event.user_id.as_ref().map(|u| u.to_string()).unwrap_or_else(|| "_none".to_owned());

        let day = ts_ns_to_day(event.ts_ns);
        groups.entry((user_key, day)).or_default().push(event);
    }

    groups
}

/// Convert nanosecond timestamp to `YYYY-MM-DD` string.
fn ts_ns_to_day(ts_ns: i64) -> String {
    let secs = ts_ns / 1_000_000_000;
    let days_since_epoch = secs / 86400;
    // Simple date calculation without external crate.
    let (y, m, d) = civil_from_days(days_since_epoch);
    format!("{y:04}-{m:02}-{d:02}")
}

/// Convert days since Unix epoch to (year, month, day).
/// Adapted from Howard Hinnant's algorithm.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u32;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y, m, d)
}

fn now_epoch_secs() -> i64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs()
        as i64
}

/// Build an S3 object store client.
fn build_s3_client(
    config: &ArchiveS3Config,
    rt: tokio::runtime::Handle,
) -> Result<Box<dyn ObjectStore>, StoreError> {
    use object_store::aws::AmazonS3Builder;

    let _guard = rt.enter();

    let mut builder = AmazonS3Builder::new()
        .with_bucket_name(&config.bucket)
        .with_endpoint(&config.endpoint)
        .with_region(&config.region)
        .with_access_key_id(&config.access_key_id)
        .with_secret_access_key(&config.secret_access_key)
        .with_allow_http(config.endpoint.starts_with("http://"));

    if config.path_style {
        builder = builder.with_virtual_hosted_style_request(false);
    }

    let store = builder.build().map_err(|e| StoreError::ArchiveS3(format!("build client: {e}")))?;

    Ok(Box::new(store))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ts_ns_to_day_known_dates() {
        // 2024-01-15 00:00:00 UTC = 1705276800 seconds
        let ts = 1_705_276_800_000_000_000i64;
        assert_eq!(ts_ns_to_day(ts), "2024-01-15");

        // 2023-12-31 23:59:59 UTC
        let ts = 1_704_067_199_000_000_000i64;
        assert_eq!(ts_ns_to_day(ts), "2023-12-31");

        // Unix epoch
        assert_eq!(ts_ns_to_day(0), "1970-01-01");
    }

    #[test]
    fn group_events_by_user_and_day() {
        use keplor_core::*;
        use smol_str::SmolStr;

        let make = |user: Option<&str>, ts_ns: i64| -> LlmEvent {
            LlmEvent {
                id: EventId::new(),
                ts_ns,
                user_id: user.map(UserId::from),
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
                usage: Usage::default(),
                cost_nanodollars: 0,
                latency: Latencies::default(),
                flags: EventFlags::empty(),
                error: None,
                request_sha256: [0u8; 32],
                response_sha256: [0u8; 32],
                client_ip: None,
                user_agent: None,
                request_id: None,
                trace_id: None,
                source: None,
                ingested_at: 0,
                metadata: None,
                tier: SmolStr::new("free"),
            }
        };

        let events = vec![
            make(Some("alice"), 1_705_276_800_000_000_000), // 2024-01-15
            make(Some("alice"), 1_705_276_801_000_000_000), // 2024-01-15
            make(Some("bob"), 1_705_276_800_000_000_000),   // 2024-01-15
            make(None, 1_705_363_200_000_000_000),          // 2024-01-16
        ];

        let groups = group_events(events);
        assert_eq!(groups.len(), 3);
        assert_eq!(groups[&("alice".to_owned(), "2024-01-15".to_owned())].len(), 2);
        assert_eq!(groups[&("bob".to_owned(), "2024-01-15".to_owned())].len(), 1);
        assert_eq!(groups[&("_none".to_owned(), "2024-01-16".to_owned())].len(), 1);
    }
}
