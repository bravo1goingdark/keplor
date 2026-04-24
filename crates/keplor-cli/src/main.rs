//! The `keplor` binary — LLM log aggregation pipeline.

#![cfg_attr(test, allow(clippy::unwrap_used, clippy::expect_used))]

#[cfg(feature = "mimalloc")]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Parser;

/// Keplor — LLM log aggregation pipeline.
#[derive(Parser)]
#[command(name = "keplor", version, about)]
enum Cli {
    /// Start the ingestion server.
    Run {
        /// Path to config file (TOML).
        #[arg(short, long, default_value = "keplor.toml")]
        config: PathBuf,
        /// Emit structured JSON logs (for log aggregation systems).
        #[arg(long)]
        json_logs: bool,
    },
    /// Run database migrations.
    Migrate {
        /// Path to the SQLite database.
        #[arg(short, long, default_value = "keplor.db")]
        db: PathBuf,
    },
    /// Query stored events.
    Query {
        /// Filter by user id.
        #[arg(long)]
        user_id: Option<String>,
        /// Filter by model name.
        #[arg(long)]
        model: Option<String>,
        /// Filter by provider.
        #[arg(long)]
        provider: Option<String>,
        /// Filter by ingestion source.
        #[arg(long)]
        source: Option<String>,
        /// Maximum results.
        #[arg(long, default_value = "20")]
        limit: u32,
        /// Path to the SQLite database.
        #[arg(short, long, default_value = "keplor.db")]
        db: PathBuf,
    },
    /// Print storage statistics.
    Stats {
        /// Path to the SQLite database.
        #[arg(short, long, default_value = "keplor.db")]
        db: PathBuf,
    },
    /// Delete events older than a threshold.
    Gc {
        /// Delete events older than this many days.
        #[arg(long)]
        older_than_days: u32,
        /// Path to the SQLite database.
        #[arg(short, long, default_value = "keplor.db")]
        db: PathBuf,
    },
    /// Backfill daily rollups from stored events.
    Rollup {
        /// Number of past days to roll up (including today).
        #[arg(long, default_value = "30")]
        days: u32,
        /// Path to the SQLite database.
        #[arg(short, long, default_value = "keplor.db")]
        db: PathBuf,
    },
    /// Archive old events to S3/R2 (requires --features s3).
    #[cfg(feature = "s3")]
    Archive {
        /// Path to config file (for S3 credentials).
        #[arg(short, long, default_value = "keplor.toml")]
        config: PathBuf,
        /// Archive events older than this many days (overrides config).
        #[arg(long)]
        older_than_days: Option<u32>,
    },
    /// Show archive status and manifest summary.
    ArchiveStatus {
        /// Path to the SQLite database.
        #[arg(short, long, default_value = "keplor.db")]
        db: PathBuf,
    },
    /// One-shot migration from a SQLite store into a KeplorDB data dir.
    ///
    /// Reads the source DB in chunks, converts each event via the
    /// mapping module, writes to per-tier KeplorDB engines, then copies
    /// archive manifests into the JSONL sidecar. Resumable: a
    /// checkpoint file is written after each batch, so an interrupted
    /// run picks up where it left off.
    MigrateFromSqlite {
        /// Source SQLite database.
        #[arg(long, default_value = "keplor.db")]
        source: PathBuf,
        /// Destination KeplorDB data directory.
        #[arg(long, default_value = "./keplor_data")]
        dest: PathBuf,
        /// Events per chunk.
        #[arg(long, default_value = "10000")]
        batch_size: u32,
    },
}

fn main() -> Result<()> {
    let cli = Cli::parse();

    match cli {
        Cli::Run { config, json_logs } => run_server(config, json_logs),
        Cli::Migrate { db } => migrate(db),
        Cli::Query { user_id, model, provider, source, limit, db } => {
            query(db, user_id, model, provider, source, limit)
        },
        Cli::Stats { db } => stats(db),
        Cli::Gc { older_than_days, db } => gc(db, older_than_days),
        Cli::Rollup { days, db } => rollup(db, days),
        #[cfg(feature = "s3")]
        Cli::Archive { config, older_than_days } => archive(config, older_than_days),
        Cli::ArchiveStatus { db } => archive_status(db),
        Cli::MigrateFromSqlite { source, dest, batch_size } => {
            migrate_from_sqlite(source, dest, batch_size)
        },
    }
}

fn run_server(config_path: PathBuf, json_logs: bool) -> Result<()> {
    init_tracing(json_logs);

    let config = if config_path.exists() {
        keplor_server::ServerConfig::load(&config_path)
            .with_context(|| format!("failed to load config from {}", config_path.display()))?
    } else {
        tracing::info!("no config file found, using defaults");
        keplor_server::ServerConfig::default()
    };

    config.validate().map_err(|e| anyhow::anyhow!("invalid config: {e}"))?;
    config.warn_risky_defaults();

    if config.auth.api_keys.is_empty() {
        tracing::warn!("API key authentication is DISABLED — all ingestion endpoints are open");
    }

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;

    rt.block_on(async {
        // Install Prometheus recorder (must be before any metrics calls).
        let metrics_handle = keplor_server::install_metrics_recorder();

        // Open storage.
        let store = {
            let db_path = &config.storage.db_path;
            let pool_size = config.storage.read_pool_size;

            let store = keplor_store::Store::open_with_pool_size(db_path, pool_size)
                .with_context(|| format!("failed to open db at {}", db_path.display()))?;

            Arc::new(store)
        };

        // Spawn batch writer.
        let batch_config = keplor_store::BatchConfig {
            batch_size: config.pipeline.batch_size,
            channel_capacity: config.pipeline.channel_capacity,
            ..keplor_store::BatchConfig::default()
        };
        let writer = Arc::new(keplor_store::BatchWriter::new(Arc::clone(&store), batch_config));

        // Load pricing catalog.
        let catalog = Arc::new(
            keplor_pricing::Catalog::load_bundled().context("failed to load pricing catalog")?,
        );
        tracing::info!(
            models = catalog.len(),
            version = keplor_pricing::PRICING_CATALOG_VERSION,
            "pricing catalog loaded"
        );

        // Build pipeline.
        let mut pipeline = keplor_server::Pipeline::new(store, writer, catalog)
            .with_max_db_size_mb(config.storage.max_db_size_mb);

        // Attach idempotency cache if enabled.
        if config.idempotency.enabled {
            let cache = Arc::new(keplor_server::idempotency::IdempotencyCache::new(
                config.idempotency.max_entries,
                std::time::Duration::from_secs(config.idempotency.ttl_secs),
            ));
            pipeline = pipeline.with_idempotency(cache);
            tracing::info!(
                ttl_secs = config.idempotency.ttl_secs,
                max_entries = config.idempotency.max_entries,
                "idempotency cache enabled"
            );
        }

        // Build and run server.
        let keys = keplor_server::auth::ApiKeySet::from_config(
            config.auth.api_keys.clone(),
            config.auth.api_key_entries.clone(),
            &config.retention.default_tier,
        );
        let server = keplor_server::PipelineServer::new(pipeline, keys, &config, metrics_handle)
            .context("failed to build server")?;

        tracing::info!("keplor starting");
        server.run().await.context("server error")
    })
}

fn migrate(db: PathBuf) -> Result<()> {
    init_tracing(false);
    let _store = keplor_store::Store::open(&db)
        .with_context(|| format!("failed to open/migrate db at {}", db.display()))?;
    println!("migrations applied to {}", db.display());
    Ok(())
}

fn query(
    db: PathBuf,
    user_id: Option<String>,
    model: Option<String>,
    provider: Option<String>,
    source: Option<String>,
    limit: u32,
) -> Result<()> {
    let store = keplor_store::Store::open(&db)
        .with_context(|| format!("failed to open db at {}", db.display()))?;

    let filter = keplor_store::EventFilter {
        user_id: user_id.map(smol_str::SmolStr::new),
        api_key_id: None,
        model: model.map(smol_str::SmolStr::new),
        provider: provider.map(smol_str::SmolStr::new),
        source: source.map(smol_str::SmolStr::new),
        from_ts_ns: None,
        to_ts_ns: None,
        ..Default::default()
    };

    let events = store.query(&filter, limit, None).context("query failed")?;

    if events.is_empty() {
        println!("no events found");
        return Ok(());
    }

    let w_id = events.iter().map(|e| e.id.to_string().len()).max().unwrap_or(2).max("ID".len());
    let w_provider =
        events.iter().map(|e| e.provider.id_key().len()).max().unwrap_or(8).max("PROVIDER".len());
    let w_model = events.iter().map(|e| e.model.len()).max().unwrap_or(5).max("MODEL".len());
    let w_tokens: usize = 12.max("TOKENS".len());
    let w_cost: usize = 14.max("COST ($)".len());
    let sep_len = w_id + 1 + w_provider + 1 + w_model + 1 + w_tokens + 1 + w_cost;

    println!(
        "{:<w_id$} {:<w_provider$} {:<w_model$} {:>w_tokens$} {:>w_cost$}",
        "ID", "PROVIDER", "MODEL", "TOKENS", "COST ($)",
    );
    println!("{}", "-".repeat(sep_len));

    for e in &events {
        let total_tokens = e.usage.input_tokens + e.usage.output_tokens;
        let cost_dollars = e.cost_nanodollars as f64 / 1_000_000_000.0;
        println!(
            "{:<w_id$} {:<w_provider$} {:<w_model$} {:>w_tokens$} {:>w_cost$.8}",
            e.id,
            e.provider.id_key(),
            e.model,
            total_tokens,
            cost_dollars,
        );
    }
    println!("\n{} event(s)", events.len());
    Ok(())
}

fn stats(db: PathBuf) -> Result<()> {
    let store = keplor_store::Store::open(&db)
        .with_context(|| format!("failed to open db at {}", db.display()))?;

    let filter = keplor_store::EventFilter::default();
    let events = store.query(&filter, 1, None).context("query failed")?;
    let total_events = if events.is_empty() {
        0
    } else {
        store.query(&filter, u32::MAX, None).map(|e| e.len()).unwrap_or(0)
    };

    let db_size = store.db_size_bytes().unwrap_or(0);

    println!("=== Keplor Storage Statistics ===");
    println!("Database:             {}", db.display());
    println!("Total events:         {total_events}");
    println!("Database size:        {:.2} MB", db_size as f64 / (1024.0 * 1024.0));
    Ok(())
}

fn gc(db: PathBuf, older_than_days: u32) -> Result<()> {
    init_tracing(false);
    let store = keplor_store::Store::open(&db)
        .with_context(|| format!("failed to open db at {}", db.display()))?;

    let cutoff_ns = {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .context("system clock error")?
            .as_nanos() as i64;
        now - (older_than_days as i64) * 86_400 * 1_000_000_000
    };

    let stats = store.gc_expired(cutoff_ns).context("gc failed")?;
    println!(
        "GC complete: deleted {} events (cutoff: {} days ago)",
        stats.events_deleted, older_than_days
    );
    Ok(())
}

fn rollup(db: PathBuf, days: u32) -> Result<()> {
    init_tracing(false);
    let store = keplor_store::Store::open(&db)
        .with_context(|| format!("failed to open db at {}", db.display()))?;

    let now_secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .context("system clock error")?
        .as_secs() as i64;
    let today = now_secs - (now_secs % 86400);

    for i in 0..days {
        let day = today - (i as i64) * 86400;
        store.rollup_day(day).with_context(|| format!("rollup failed for day {day}"))?;
    }

    println!("rolled up {days} days ending at {today} (epoch seconds)");
    Ok(())
}

#[cfg(feature = "s3")]
fn archive(config_path: PathBuf, older_than_days_override: Option<u32>) -> Result<()> {
    init_tracing(false);

    let config = keplor_server::ServerConfig::load(&config_path)
        .with_context(|| format!("failed to load config from {}", config_path.display()))?;

    let archive_cfg =
        config.archive.as_ref().ok_or_else(|| anyhow::anyhow!("no [archive] section in config"))?;

    let store = Arc::new(
        keplor_store::Store::open_with_pool_size(
            &config.storage.db_path,
            config.storage.read_pool_size,
        )
        .with_context(|| format!("failed to open db at {}", config.storage.db_path.display()))?,
    );

    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;

    let s3_config = keplor_store::ArchiveS3Config {
        bucket: archive_cfg.bucket.clone(),
        endpoint: archive_cfg.endpoint.clone(),
        region: archive_cfg.region.clone(),
        access_key_id: archive_cfg.access_key_id.clone(),
        secret_access_key: archive_cfg.secret_access_key.clone(),
        prefix: archive_cfg.prefix.clone(),
        path_style: archive_cfg.path_style,
    };

    let archiver = keplor_store::Archiver::new(Arc::clone(&store), &s3_config, rt.handle().clone())
        .context("failed to initialize archiver")?;

    let days = older_than_days_override.map(u64::from).unwrap_or(archive_cfg.archive_after_days);

    let cutoff_ns = {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .context("system clock error")?
            .as_nanos() as i64;
        now - (days as i64) * 86_400 * 1_000_000_000
    };

    let result = archiver
        .archive_old_events(cutoff_ns, archive_cfg.archive_batch_size)
        .context("archive failed")?;

    println!(
        "Archive complete: {} events archived, {} files uploaded, {:.2} MB compressed",
        result.events_archived,
        result.files_uploaded,
        result.compressed_bytes as f64 / (1024.0 * 1024.0),
    );
    Ok(())
}

fn archive_status(db: PathBuf) -> Result<()> {
    let store = keplor_store::Store::open(&db)
        .with_context(|| format!("failed to open db at {}", db.display()))?;

    let (files, events, bytes) = store.archive_summary().context("query archive summary")?;

    println!("=== Archive Status ===");
    println!("Database:        {}", db.display());
    println!("Archive files:   {files}");
    println!("Archived events: {events}");
    println!("Compressed size: {:.2} MB", bytes as f64 / (1024.0 * 1024.0));

    if files > 0 {
        let manifests = store.list_archives(None, None, None).unwrap_or_default();
        // Show per-user breakdown.
        let mut user_counts: std::collections::BTreeMap<String, (usize, usize)> =
            std::collections::BTreeMap::new();
        for m in &manifests {
            let entry = user_counts.entry(m.user_id.clone()).or_default();
            entry.0 += m.event_count;
            entry.1 += m.compressed_bytes;
        }
        println!("\nPer-user breakdown:");
        for (user, (count, bytes)) in &user_counts {
            println!("  {user}: {count} events, {:.2} MB", *bytes as f64 / (1024.0 * 1024.0));
        }
    }
    Ok(())
}

fn init_tracing(json: bool) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    if json {
        tracing_subscriber::fmt().json().with_env_filter(filter).init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
}

fn migrate_from_sqlite(source: PathBuf, dest: PathBuf, batch_size: u32) -> Result<()> {
    use keplor_store::kdb_store::{KdbConfig, KdbStore};
    use keplor_store::Store;

    init_tracing(false);

    anyhow::ensure!(source.exists(), "source SQLite database does not exist: {}", source.display());
    anyhow::ensure!(batch_size > 0, "batch_size must be positive");

    println!(
        "Migrating from {} → {} (batch_size = {batch_size})",
        source.display(),
        dest.display()
    );

    let src = Store::open(&source)
        .with_context(|| format!("failed to open source db at {}", source.display()))?;
    let dst = KdbStore::open(KdbConfig::new(dest.clone()))
        .with_context(|| format!("failed to open destination at {}", dest.display()))?;

    let checkpoint_path = dest.join(".migrate_from_sqlite.checkpoint");
    let stats = run_migration(&src, &dst, batch_size, &checkpoint_path, true)?;

    println!(
        "\nMigration complete:\n  events_written: {}\n  events_skipped: {}\n  \
         batches: {}\n  archive_manifests: {}",
        stats.events_written, stats.events_skipped, stats.batches, stats.manifests
    );
    Ok(())
}

/// Stats returned by [`run_migration`].
#[derive(Debug, Default)]
struct MigrateStats {
    events_written: u64,
    events_skipped: u64,
    batches: u64,
    manifests: usize,
}

/// Core migration loop — no tracing init, no global state. Tests call
/// this directly.
fn run_migration(
    src: &keplor_store::Store,
    dst: &keplor_store::kdb_store::KdbStore,
    batch_size: u32,
    checkpoint_path: &std::path::Path,
    verbose: bool,
) -> Result<MigrateStats> {
    use keplor_store::filter::{Cursor, EventFilter};

    let mut cursor = read_checkpoint(checkpoint_path)?;
    if verbose {
        if let Some(c) = cursor {
            println!("Resuming from checkpoint ts_ns={c} (events older than this are pending)");
        }
    }

    let filter = EventFilter::default();
    let mut stats = MigrateStats::default();

    loop {
        let batch = src
            .query(&filter, batch_size, cursor.map(Cursor))
            .context("failed to read chunk from source")?;
        if batch.is_empty() {
            break;
        }

        // Idempotency probe: if the first event already exists in the
        // destination (e.g. a crash that fsync'd the append but not the
        // checkpoint), skip this whole batch.
        let first_id = batch[0].id;
        let already_written =
            dst.get_event(&first_id).context("failed to probe destination")?.is_some();

        let min_ts = batch.iter().map(|e| e.ts_ns).min().unwrap_or(0);
        cursor = Some(min_ts);

        if already_written {
            stats.events_skipped += batch.len() as u64;
        } else {
            dst.append_batch(&batch).context("failed to write batch to destination")?;
            stats.events_written += batch.len() as u64;
        }
        stats.batches += 1;

        write_checkpoint(checkpoint_path, min_ts)?;

        if verbose && stats.batches % 10 == 0 {
            println!(
                "  {} batches, {} written, {} skipped (already present), cursor ts_ns={}",
                stats.batches, stats.events_written, stats.events_skipped, min_ts
            );
        }
    }

    dst.wal_checkpoint().context("failed to flush destination WAL")?;

    // Manifest migration only when the SQLite source was built with
    // the `s3` feature — that's the build that owns
    // `insert_archive_manifest` on the source side. Without it there
    // are no manifests to migrate.
    #[cfg(feature = "s3")]
    {
        let manifests = src.list_archives(None, None, None).context("failed to list archives")?;
        stats.manifests = manifests.len();
        for m in manifests {
            dst.insert_archive_manifest(&m).context("failed to insert manifest")?;
        }
    }
    #[cfg(not(feature = "s3"))]
    {
        let _ = src;
        let _ = &stats.manifests;
    }

    if checkpoint_path.exists() {
        std::fs::remove_file(checkpoint_path).ok();
    }

    Ok(stats)
}

fn read_checkpoint(path: &std::path::Path) -> Result<Option<i64>> {
    if !path.exists() {
        return Ok(None);
    }
    let s = std::fs::read_to_string(path)
        .with_context(|| format!("failed to read checkpoint at {}", path.display()))?;
    let ts: i64 = s
        .trim()
        .parse()
        .with_context(|| format!("invalid checkpoint content in {}", path.display()))?;
    Ok(Some(ts))
}

fn write_checkpoint(path: &std::path::Path, ts_ns: i64) -> Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).ok();
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, ts_ns.to_string())
        .with_context(|| format!("failed to write checkpoint at {}", tmp.display()))?;
    std::fs::rename(&tmp, path)
        .with_context(|| format!("failed to rename checkpoint to {}", path.display()))?;
    Ok(())
}

#[cfg(test)]
mod migration_tests {
    use super::*;
    use keplor_core::{
        ApiKeyId, EventFlags, EventId, Latencies, LlmEvent, Provider, RouteId, Usage, UserId,
    };
    use keplor_store::kdb_store::{KdbConfig, KdbStore};
    #[cfg(feature = "s3")]
    use keplor_store::ArchiveManifest;
    use keplor_store::Store;
    use smol_str::SmolStr;

    fn sample_event(tier: &str, ts_ns: i64, user: &str, cost: i64) -> LlmEvent {
        LlmEvent {
            id: EventId::new(),
            ts_ns,
            user_id: Some(UserId::from(user)),
            api_key_id: Some(ApiKeyId::from("key_test")),
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
            latency: Latencies { ttft_ms: Some(40), total_ms: 300, time_to_close_ms: None },
            flags: EventFlags::empty(),
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

    #[cfg(feature = "s3")]
    #[test]
    fn end_to_end_migration_moves_events_and_manifests() {
        let tmp = tempfile::tempdir().unwrap();
        let src_path = tmp.path().join("src.db");
        let dst_path = tmp.path().join("dest");
        let checkpoint = tmp.path().join(".checkpoint");

        // Seed a SQLite source with 25 events across 2 tiers and one
        // manifest.
        {
            let src = Store::open(&src_path).unwrap();
            let base_ts = 1_700_000_000_000_000_000i64;
            for i in 0..25 {
                let tier = if i % 2 == 0 { "pro" } else { "free" };
                src.append_event(&sample_event(tier, base_ts + i * 1_000_000, "alice", 1_000))
                    .unwrap();
            }
            src.insert_archive_manifest(&ArchiveManifest {
                archive_id: "a1".into(),
                user_id: "alice".into(),
                day: "2026-04-01".into(),
                s3_key: "prefix/alice/2026-04-01.jsonl.zstd".into(),
                event_count: 10,
                min_ts_ns: 100,
                max_ts_ns: 200,
                compressed_bytes: 4096,
                created_at: 1_700_000_000,
            })
            .unwrap();
        }

        let src = Store::open(&src_path).unwrap();
        let dst = KdbStore::open(KdbConfig::new(dst_path.clone())).unwrap();

        let stats = run_migration(&src, &dst, 10, &checkpoint, false).unwrap();
        assert_eq!(stats.events_written, 25);
        assert_eq!(stats.events_skipped, 0);
        assert_eq!(stats.batches, 3); // 10 + 10 + 5
        assert_eq!(stats.manifests, 1);

        // Quota matches source (both tiers contribute).
        let q = dst.quota_summary(Some("alice"), None, 0).unwrap();
        assert_eq!(q.event_count, 25);
        assert_eq!(q.cost_nanodollars, 25_000);

        // Manifest made it over.
        let ms = dst.list_archives(Some("alice"), None, None, 100, 0).unwrap();
        assert_eq!(ms.len(), 1);
        assert_eq!(ms[0].archive_id, "a1");

        // Checkpoint cleaned up on success.
        assert!(!checkpoint.exists());
    }

    #[test]
    fn end_to_end_migration_moves_events_across_tiers() {
        // Non-s3 variant of the above — exercises the core event
        // migration path on every build.
        let tmp = tempfile::tempdir().unwrap();
        let src_path = tmp.path().join("src.db");
        let dst_path = tmp.path().join("dest");
        let checkpoint = tmp.path().join(".checkpoint");

        {
            let src = Store::open(&src_path).unwrap();
            let base_ts = 1_700_000_000_000_000_000i64;
            for i in 0..25 {
                let tier = if i % 2 == 0 { "pro" } else { "free" };
                src.append_event(&sample_event(tier, base_ts + i * 1_000_000, "alice", 1_000))
                    .unwrap();
            }
        }

        let src = Store::open(&src_path).unwrap();
        let dst = KdbStore::open(KdbConfig::new(dst_path)).unwrap();

        let stats = run_migration(&src, &dst, 10, &checkpoint, false).unwrap();
        assert_eq!(stats.events_written, 25);
        assert_eq!(stats.events_skipped, 0);
        assert_eq!(stats.batches, 3);

        let q = dst.quota_summary(Some("alice"), None, 0).unwrap();
        assert_eq!(q.event_count, 25);
        assert_eq!(q.cost_nanodollars, 25_000);

        assert!(!checkpoint.exists());
    }

    #[test]
    fn resume_from_checkpoint_skips_already_written_batches() {
        let tmp = tempfile::tempdir().unwrap();
        let src_path = tmp.path().join("src.db");
        let dst_path = tmp.path().join("dest");
        let checkpoint = tmp.path().join(".checkpoint");

        let base_ts = 1_700_000_000_000_000_000i64;
        {
            let src = Store::open(&src_path).unwrap();
            for i in 0..20 {
                src.append_event(&sample_event("pro", base_ts + i * 1_000_000, "alice", 500))
                    .unwrap();
            }
        }

        let src = Store::open(&src_path).unwrap();
        let dst = KdbStore::open(KdbConfig::new(dst_path)).unwrap();

        // First pass — run to completion.
        let stats = run_migration(&src, &dst, 5, &checkpoint, false).unwrap();
        assert_eq!(stats.events_written, 20);
        assert_eq!(stats.events_skipped, 0);

        // Second pass — checkpoint was cleaned up, but segments exist
        // in dst. The first-id idempotency probe must catch each batch
        // and mark everything as skipped.
        let stats = run_migration(&src, &dst, 5, &checkpoint, false).unwrap();
        assert_eq!(stats.events_written, 0);
        assert_eq!(stats.events_skipped, 20);
        assert_eq!(stats.batches, 4);
    }

    #[test]
    fn checkpoint_written_per_batch_and_cleaned_on_success() {
        let tmp = tempfile::tempdir().unwrap();
        let src_path = tmp.path().join("src.db");
        let dst_path = tmp.path().join("dest");
        let checkpoint = tmp.path().join(".checkpoint");

        {
            let src = Store::open(&src_path).unwrap();
            for i in 0..3 {
                src.append_event(&sample_event("pro", 1_700_000_000_000_000_000 + i, "alice", 10))
                    .unwrap();
            }
        }

        let src = Store::open(&src_path).unwrap();
        let dst = KdbStore::open(KdbConfig::new(dst_path)).unwrap();

        let stats = run_migration(&src, &dst, 5, &checkpoint, false).unwrap();
        assert_eq!(stats.events_written, 3);
        // Success path deletes the checkpoint.
        assert!(!checkpoint.exists());
    }

    #[test]
    fn empty_source_is_noop() {
        let tmp = tempfile::tempdir().unwrap();
        let src_path = tmp.path().join("src.db");
        let dst_path = tmp.path().join("dest");
        let checkpoint = tmp.path().join(".checkpoint");

        let _ = Store::open(&src_path).unwrap();
        let src = Store::open(&src_path).unwrap();
        let dst = KdbStore::open(KdbConfig::new(dst_path)).unwrap();

        let stats = run_migration(&src, &dst, 10, &checkpoint, false).unwrap();
        assert_eq!(stats.events_written, 0);
        assert_eq!(stats.batches, 0);
        assert_eq!(stats.manifests, 0);
    }
}
