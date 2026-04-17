//! Versioned SQLite schema migrations.

use rusqlite::Connection;

use crate::error::StoreError;

/// All migrations in order.  Each entry is `(version, sql)`.
static MIGRATIONS: &[(u32, &str)] = &[(1, MIGRATION_0001)];

const MIGRATION_0001: &str = r"
CREATE TABLE IF NOT EXISTS schema_version(
  version INTEGER PRIMARY KEY,
  applied_at INTEGER NOT NULL
);

CREATE TABLE llm_events (
  id BLOB PRIMARY KEY,
  ts_ns INTEGER NOT NULL,
  user_id TEXT, api_key_id TEXT, org_id TEXT, project_id TEXT, route_id TEXT,
  provider TEXT NOT NULL, model TEXT NOT NULL, model_family TEXT,
  endpoint TEXT NOT NULL, method TEXT NOT NULL, http_status INTEGER,
  input_tokens INTEGER NOT NULL DEFAULT 0,
  output_tokens INTEGER NOT NULL DEFAULT 0,
  cache_read_input_tokens INTEGER NOT NULL DEFAULT 0,
  cache_creation_input_tokens INTEGER NOT NULL DEFAULT 0,
  reasoning_tokens INTEGER NOT NULL DEFAULT 0,
  audio_input_tokens INTEGER NOT NULL DEFAULT 0,
  audio_output_tokens INTEGER NOT NULL DEFAULT 0,
  image_tokens INTEGER NOT NULL DEFAULT 0,
  tool_use_tokens INTEGER NOT NULL DEFAULT 0,
  cost_nanodollars INTEGER NOT NULL DEFAULT 0,
  latency_ttft_ms INTEGER, latency_total_ms INTEGER, time_to_close_ms INTEGER,
  streaming INTEGER NOT NULL, tool_calls INTEGER NOT NULL,
  reasoning INTEGER NOT NULL, stream_incomplete INTEGER NOT NULL,
  error_type TEXT, error_message TEXT,
  request_sha256 BLOB NOT NULL, response_sha256 BLOB NOT NULL,
  request_blob_id BLOB, response_blob_id BLOB,
  client_ip TEXT, user_agent TEXT, request_id TEXT, trace_id TEXT
) STRICT;

CREATE INDEX idx_events_ts ON llm_events(ts_ns);
CREATE INDEX idx_events_user_ts ON llm_events(user_id, ts_ns);
CREATE INDEX idx_events_key_ts ON llm_events(api_key_id, ts_ns);
CREATE INDEX idx_events_model_ts ON llm_events(model, ts_ns);

CREATE TABLE payload_blobs (
  sha256 BLOB PRIMARY KEY,
  component_type TEXT NOT NULL,
  provider TEXT NOT NULL,
  compression TEXT NOT NULL,
  dict_id TEXT,
  uncompressed_size INTEGER NOT NULL,
  compressed_size INTEGER NOT NULL,
  refcount INTEGER NOT NULL DEFAULT 1,
  hit_count INTEGER NOT NULL DEFAULT 0,
  data BLOB NOT NULL,
  first_seen_at INTEGER NOT NULL
) STRICT;

CREATE TABLE event_components (
  event_id BLOB NOT NULL REFERENCES llm_events(id),
  component_type TEXT NOT NULL,
  blob_sha256 BLOB NOT NULL REFERENCES payload_blobs(sha256),
  PRIMARY KEY(event_id, component_type)
) STRICT;

CREATE TABLE zstd_dicts (
  id TEXT PRIMARY KEY,
  provider TEXT NOT NULL,
  component_type TEXT NOT NULL,
  sample_count INTEGER NOT NULL,
  created_at INTEGER NOT NULL,
  data BLOB NOT NULL
) STRICT;

CREATE TABLE daily_rollups (
  day INTEGER NOT NULL, user_id TEXT, api_key_id TEXT, model TEXT,
  event_count INTEGER, input_tokens INTEGER, output_tokens INTEGER,
  cost_nanodollars INTEGER,
  PRIMARY KEY(day, user_id, api_key_id, model)
) STRICT;
";

/// Apply all unapplied migrations.
pub(crate) fn migrate(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "CREATE TABLE IF NOT EXISTS schema_version(
           version INTEGER PRIMARY KEY,
           applied_at INTEGER NOT NULL
         )",
    )
    .map_err(|e| StoreError::Migration { version: 0, reason: e.to_string() })?;

    let current: u32 = conn
        .query_row("SELECT COALESCE(MAX(version), 0) FROM schema_version", [], |r| r.get(0))
        .unwrap_or(0);

    for &(version, sql) in MIGRATIONS {
        if version <= current {
            continue;
        }
        conn.execute_batch(sql)
            .map_err(|e| StoreError::Migration { version, reason: e.to_string() })?;
        conn.execute(
            "INSERT INTO schema_version(version, applied_at) VALUES(?1, strftime('%s','now'))",
            [version],
        )
        .map_err(|e| StoreError::Migration { version, reason: e.to_string() })?;
        tracing::info!(version, "applied migration");
    }

    Ok(())
}

/// Apply recommended pragmas for performance.
pub(crate) fn apply_pragmas(conn: &Connection) -> Result<(), StoreError> {
    conn.execute_batch(
        "PRAGMA journal_mode=WAL;
         PRAGMA synchronous=NORMAL;
         PRAGMA mmap_size=268435456;
         PRAGMA busy_timeout=5000;",
    )?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn migrate_fresh_db() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        let ver: u32 =
            conn.query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0)).unwrap();
        assert_eq!(ver, 1);
    }

    #[test]
    fn migrate_is_idempotent() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        migrate(&conn).unwrap();
        let ver: u32 =
            conn.query_row("SELECT MAX(version) FROM schema_version", [], |r| r.get(0)).unwrap();
        assert_eq!(ver, 1);
    }

    #[test]
    fn tables_exist_after_migration() {
        let conn = Connection::open_in_memory().unwrap();
        migrate(&conn).unwrap();
        for table in
            &["llm_events", "payload_blobs", "event_components", "zstd_dicts", "daily_rollups"]
        {
            let exists: bool = conn
                .query_row(
                    "SELECT 1 FROM sqlite_master WHERE type='table' AND name=?1",
                    [table],
                    |_| Ok(true),
                )
                .unwrap_or(false);
            assert!(exists, "table {table} should exist");
        }
    }
}
