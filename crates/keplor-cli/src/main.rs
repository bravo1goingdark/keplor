//! The `keplor` binary — LLM log aggregation pipeline.

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
        let store = Arc::new(
            keplor_store::Store::open_with_pool_size(
                &config.storage.db_path,
                config.storage.read_pool_size,
            )
            .with_context(|| {
                format!("failed to open db at {}", config.storage.db_path.display())
            })?,
        );

        // Spawn batch writer.
        let batch_config = keplor_store::BatchConfig {
            batch_size: config.pipeline.batch_size,
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
        let keys = keplor_server::auth::ApiKeySet::new(config.auth.api_keys.clone());
        let server = keplor_server::PipelineServer::new(pipeline, keys, &config, metrics_handle);

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
        // Count by querying with a large limit.
        store.query(&filter, u32::MAX, None).map(|e| e.len()).unwrap_or(0)
    };

    let blob_count = store.blob_count().unwrap_or(0);
    let compressed = store.total_compressed_bytes().unwrap_or(0);
    let uncompressed = store.total_uncompressed_bytes().unwrap_or(0);

    println!("=== Keplor Storage Statistics ===");
    println!("Database:             {}", db.display());
    println!("Total events:         {total_events}");
    println!("Unique blobs:         {blob_count}");
    println!("Compressed size:      {} bytes", compressed);
    println!("Uncompressed size:    {} bytes", uncompressed);
    if compressed > 0 {
        println!("Compression ratio:    {:.1}x", uncompressed as f64 / compressed as f64);
    }
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
        "GC complete: deleted {} events, {} orphaned blobs (cutoff: {} days ago)",
        stats.events_deleted, stats.blobs_deleted, older_than_days
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

fn init_tracing(json: bool) {
    use tracing_subscriber::EnvFilter;
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    if json {
        tracing_subscriber::fmt().json().with_env_filter(filter).init();
    } else {
        tracing_subscriber::fmt().with_env_filter(filter).init();
    }
}
