//! The `keplor` binary — LLM log aggregation pipeline.

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
