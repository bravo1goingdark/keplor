//! Append-only JSONL sidecar for archive manifests.
//!
//! KeplorDB has no schema for manifest rows, so we keep them in a
//! small file beside the engine directories. Writes go through
//! `insert` (append + fsync); reads are served from an in-memory
//! index populated on open.
//!
//! Format: one `serde_json` record per line, `\n` terminator. Records
//! are appended atomically — a partial trailing line is tolerated by
//! the reader and discarded.

use std::collections::BTreeMap;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, BufWriter, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::error::StoreError;
use crate::store::ArchiveManifest;

/// On-disk line record. Same fields as [`ArchiveManifest`] — the
/// indirection lets us add forwards-compatible fields later without
/// churning the public type.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct Record {
    archive_id: String,
    user_id: String,
    day: String,
    s3_key: String,
    event_count: usize,
    min_ts_ns: i64,
    max_ts_ns: i64,
    compressed_bytes: usize,
    created_at: i64,
}

impl From<ArchiveManifest> for Record {
    fn from(m: ArchiveManifest) -> Self {
        Self {
            archive_id: m.archive_id,
            user_id: m.user_id,
            day: m.day,
            s3_key: m.s3_key,
            event_count: m.event_count,
            min_ts_ns: m.min_ts_ns,
            max_ts_ns: m.max_ts_ns,
            compressed_bytes: m.compressed_bytes,
            created_at: m.created_at,
        }
    }
}

impl From<Record> for ArchiveManifest {
    fn from(r: Record) -> Self {
        Self {
            archive_id: r.archive_id,
            user_id: r.user_id,
            day: r.day,
            s3_key: r.s3_key,
            event_count: r.event_count,
            min_ts_ns: r.min_ts_ns,
            max_ts_ns: r.max_ts_ns,
            compressed_bytes: r.compressed_bytes,
            created_at: r.created_at,
        }
    }
}

/// Index key — `(user_id, day)` scopes lookups to a single partition.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct IndexKey {
    user_id: String,
    day: String,
}

/// In-memory manifest store backed by an append-only JSONL log.
#[derive(Debug)]
pub struct ManifestStore {
    path: PathBuf,
    /// `(user_id, day)` → manifests within that partition.
    entries: BTreeMap<IndexKey, Vec<ArchiveManifest>>,
}

impl ManifestStore {
    /// Open (or create) the manifest log at `path`.
    pub fn open(path: &Path) -> Result<Self, StoreError> {
        let mut entries: BTreeMap<IndexKey, Vec<ArchiveManifest>> = BTreeMap::new();
        if path.exists() {
            let file = File::open(path)?;
            for line in BufReader::new(file).lines() {
                let line = match line {
                    Ok(l) if !l.is_empty() => l,
                    // Tolerate truncated trailing line (e.g. partial write
                    // on crash) — skip and keep parsing.
                    _ => continue,
                };
                let Ok(rec) = serde_json::from_str::<Record>(&line) else {
                    continue;
                };
                let m: ArchiveManifest = rec.into();
                entries
                    .entry(IndexKey { user_id: m.user_id.clone(), day: m.day.clone() })
                    .or_default()
                    .push(m);
            }
        }
        Ok(Self { path: path.to_path_buf(), entries })
    }

    /// Append a manifest and fsync the log.
    pub fn insert(&mut self, m: ArchiveManifest) -> Result<(), StoreError> {
        let rec: Record = m.clone().into();
        let mut line = serde_json::to_string(&rec).map_err(|e| StoreError::Other(e.to_string()))?;
        line.push('\n');
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let file = OpenOptions::new().create(true).append(true).open(&self.path)?;
        let mut w = BufWriter::new(file);
        w.write_all(line.as_bytes())?;
        w.flush()?;
        w.into_inner().map_err(|e| StoreError::Io(e.into_error()))?.sync_data()?;

        self.entries
            .entry(IndexKey { user_id: m.user_id.clone(), day: m.day.clone() })
            .or_default()
            .push(m);
        Ok(())
    }

    /// True iff any manifest's `[min_ts_ns, max_ts_ns]` overlaps the
    /// requested window.
    pub fn any_overlapping(&self, user_id: Option<&str>, from: i64, to: i64) -> bool {
        for (k, v) in &self.entries {
            if let Some(u) = user_id {
                if k.user_id != u {
                    continue;
                }
            }
            for m in v {
                if m.max_ts_ns >= from && m.min_ts_ns <= to {
                    return true;
                }
            }
        }
        false
    }

    /// Return manifests matching the filter, paginated.
    pub fn list(
        &self,
        user_id: Option<&str>,
        from_ts_ns: Option<i64>,
        to_ts_ns: Option<i64>,
        limit: u32,
        offset: u32,
    ) -> Vec<ArchiveManifest> {
        let mut out: Vec<ArchiveManifest> = Vec::new();
        for (k, v) in &self.entries {
            if let Some(u) = user_id {
                if k.user_id != u {
                    continue;
                }
            }
            for m in v {
                if let Some(from) = from_ts_ns {
                    if m.max_ts_ns < from {
                        continue;
                    }
                }
                if let Some(to) = to_ts_ns {
                    if m.min_ts_ns > to {
                        continue;
                    }
                }
                out.push(m.clone());
            }
        }
        out.sort_by(|a, b| a.min_ts_ns.cmp(&b.min_ts_ns));
        out.into_iter().skip(offset as usize).take(limit as usize).collect()
    }

    /// `(manifest count, total compressed bytes, oldest min_ts_ns)`.
    pub fn summary(&self) -> (usize, usize, i64) {
        let mut count = 0usize;
        let mut bytes = 0usize;
        let mut oldest = i64::MAX;
        for v in self.entries.values() {
            for m in v {
                count += 1;
                bytes = bytes.saturating_add(m.compressed_bytes);
                if m.min_ts_ns < oldest {
                    oldest = m.min_ts_ns;
                }
            }
        }
        let oldest = if count == 0 { 0 } else { oldest };
        (count, bytes, oldest)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn make_manifest(user: &str, day: &str, min_ts: i64, max_ts: i64) -> ArchiveManifest {
        ArchiveManifest {
            archive_id: format!("{user}-{day}"),
            user_id: user.to_owned(),
            day: day.to_owned(),
            s3_key: format!("prefix/user_id={user}/day={day}/x.jsonl.zstd"),
            event_count: 10,
            min_ts_ns: min_ts,
            max_ts_ns: max_ts,
            compressed_bytes: 1234,
            created_at: 1_700_000_000,
        }
    }

    #[test]
    fn append_then_reopen_preserves_entries() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mf.jsonl");
        {
            let mut store = ManifestStore::open(&path).unwrap();
            store.insert(make_manifest("alice", "2026-04-01", 100, 200)).unwrap();
            store.insert(make_manifest("alice", "2026-04-02", 300, 400)).unwrap();
        }
        let store = ManifestStore::open(&path).unwrap();
        let all = store.list(None, None, None, 100, 0);
        assert_eq!(all.len(), 2);
    }

    #[test]
    fn overlap_query_respects_window() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mf.jsonl");
        let mut store = ManifestStore::open(&path).unwrap();
        store.insert(make_manifest("alice", "2026-04-01", 100, 200)).unwrap();

        assert!(store.any_overlapping(Some("alice"), 50, 150));
        assert!(store.any_overlapping(Some("alice"), 150, 250));
        assert!(!store.any_overlapping(Some("alice"), 300, 400));
        assert!(!store.any_overlapping(Some("bob"), 100, 200));
    }

    #[test]
    fn summary_on_empty_store() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mf.jsonl");
        let store = ManifestStore::open(&path).unwrap();
        assert_eq!(store.summary(), (0, 0, 0));
    }

    #[test]
    fn summary_counts_and_sums_correctly() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mf.jsonl");
        let mut store = ManifestStore::open(&path).unwrap();
        store.insert(make_manifest("alice", "2026-04-01", 100, 200)).unwrap();
        store.insert(make_manifest("bob", "2026-04-01", 50, 150)).unwrap();
        let (n, b, oldest) = store.summary();
        assert_eq!(n, 2);
        assert_eq!(b, 2468);
        assert_eq!(oldest, 50);
    }

    #[test]
    fn tolerates_truncated_trailing_line() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("mf.jsonl");
        {
            let mut store = ManifestStore::open(&path).unwrap();
            store.insert(make_manifest("alice", "2026-04-01", 100, 200)).unwrap();
        }
        // Append a garbled line to simulate a partial write.
        {
            let mut f = OpenOptions::new().append(true).open(&path).unwrap();
            f.write_all(b"{not-json\n").unwrap();
        }
        let store = ManifestStore::open(&path).unwrap();
        let all = store.list(None, None, None, 100, 0);
        assert_eq!(all.len(), 1);
    }
}
