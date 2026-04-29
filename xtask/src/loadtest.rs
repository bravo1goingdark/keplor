//! `cargo xtask loadtest` — sustained-rate load harness for keplor.
//!
//! Drives `POST /v1/events` at a configured rate-per-second for a
//! fixed duration, records per-request latency, prints a percentile
//! report, and optionally compares against a saved baseline so CI
//! can gate on regressions.

use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use anyhow::{anyhow, bail, Context, Result};
use hdrhistogram::Histogram;
use serde::{Deserialize, Serialize};
use tokio::sync::Mutex;

#[derive(Debug)]
pub struct LoadtestArgs {
    /// Target requests per second (aggregate across all workers).
    pub rate: u64,
    /// How long to sustain the rate.
    pub duration: Duration,
    /// Number of concurrent worker tasks.
    pub concurrency: usize,
    /// Base URL of the keplor server (e.g. `http://127.0.0.1:8080`).
    pub target: String,
    /// Optional baseline file. When set, compare p99 against it and
    /// exit non-zero on >20 % regression.
    pub baseline: Option<PathBuf>,
    /// API key (sent as `Authorization: Bearer <key>`). Optional.
    pub api_key: Option<String>,
    /// When `false`, append `?durable=false` to the URL — the server
    /// treats the request as fire-and-forget and returns 202 without
    /// awaiting the BatchWriter flush. Default `true`.
    pub durable: bool,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct LoadtestResult {
    pub rate_target: u64,
    pub rate_achieved: f64,
    pub duration_secs: f64,
    pub total_requests: u64,
    pub success_count: u64,
    pub error_count: u64,
    pub p50_us: u64,
    pub p95_us: u64,
    pub p99_us: u64,
    pub p999_us: u64,
    pub p99_9_us: u64, // alias for p999 — kept for legacy callers
    pub queue_depth_max: u64,
}

impl LoadtestResult {
    fn render(&self) {
        println!("\n=== loadtest result ===");
        println!("target rate:   {} req/s", self.rate_target);
        println!("achieved rate: {:.0} req/s", self.rate_achieved);
        println!("duration:      {:.2}s", self.duration_secs);
        println!(
            "requests:      {} ({} ok / {} err)",
            self.total_requests, self.success_count, self.error_count
        );
        println!("latency p50:   {:>8} µs", self.p50_us);
        println!("latency p95:   {:>8} µs", self.p95_us);
        println!("latency p99:   {:>8} µs", self.p99_us);
        println!("latency p99.9: {:>8} µs", self.p999_us);
        println!("queue max:     {}", self.queue_depth_max);
    }
}

pub fn run(args: LoadtestArgs) -> Result<()> {
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .context("failed to build tokio runtime")?;
    let result = rt.block_on(run_async(&args))?;
    result.render();

    if let Some(baseline_path) = &args.baseline {
        gate_on_baseline(&result, baseline_path)?;
    }
    Ok(())
}

async fn run_async(args: &LoadtestArgs) -> Result<LoadtestResult> {
    let client = reqwest::Client::builder()
        .pool_max_idle_per_host(args.concurrency * 2)
        .timeout(Duration::from_secs(10))
        .build()
        .context("failed to build http client")?;

    // Histogram of latencies in microseconds; max value 60 s.
    let hist = Arc::new(Mutex::new(
        Histogram::<u64>::new_with_bounds(1, 60_000_000, 3)
            .map_err(|e| anyhow!("invalid histogram bounds: {e}"))?,
    ));
    let success = Arc::new(AtomicU64::new(0));
    let errors = Arc::new(AtomicU64::new(0));
    let queue_max = Arc::new(AtomicU64::new(0));

    let url = if args.durable {
        format!("{}/v1/events", args.target.trim_end_matches('/'))
    } else {
        format!("{}/v1/events?durable=false", args.target.trim_end_matches('/'))
    };
    let auth = args.api_key.as_ref().map(|k| format!("Bearer {k}"));

    let payload = serde_json::json!({
        "model": "gpt-4o",
        "provider": "openai",
        "usage": {
            "input_tokens": 100,
            "output_tokens": 50,
        },
        "latency": { "ttft_ms": 25, "total_ms": 300 },
        "http_status": 200,
        "user_id": "loadtest_user",
        "endpoint": "/v1/chat/completions",
    });

    // Per-worker token bucket: each worker gets a share of the
    // aggregate rate.
    let per_worker_rate = args.rate.div_ceil(args.concurrency as u64).max(1);
    let interval_per_worker = Duration::from_nanos(1_000_000_000u64 / per_worker_rate);
    let deadline = Instant::now() + args.duration;

    let mut workers = Vec::with_capacity(args.concurrency);
    for _ in 0..args.concurrency {
        let client = client.clone();
        let url = url.clone();
        let auth = auth.clone();
        let payload = payload.clone();
        let hist = Arc::clone(&hist);
        let success = Arc::clone(&success);
        let errors = Arc::clone(&errors);
        workers.push(tokio::spawn(async move {
            let mut next = Instant::now();
            while Instant::now() < deadline {
                if Instant::now() < next {
                    tokio::time::sleep_until(tokio::time::Instant::from_std(next)).await;
                }
                next += interval_per_worker;

                let mut req = client.post(&url).json(&payload);
                if let Some(a) = auth.as_deref() {
                    req = req.header("authorization", a);
                }
                let started = Instant::now();
                match req.send().await {
                    Ok(resp) if matches!(resp.status().as_u16(), 200..=299) => {
                        let elapsed_us = started.elapsed().as_micros() as u64;
                        let mut h = hist.lock().await;
                        let _ = h.record(elapsed_us);
                        success.fetch_add(1, Ordering::Relaxed);
                    },
                    Ok(_) | Err(_) => {
                        errors.fetch_add(1, Ordering::Relaxed);
                    },
                }
            }
        }));
    }

    // Concurrent /health poller — samples queue depth every 250 ms so
    // the report reflects back-pressure, not just steady-state.
    let queue_max_poller = Arc::clone(&queue_max);
    let target_for_poll = args.target.clone();
    let client_for_poll = client.clone();
    let poller = tokio::spawn(async move {
        let url = format!("{}/health", target_for_poll.trim_end_matches('/'));
        let mut tick = tokio::time::interval(Duration::from_millis(250));
        while Instant::now() < deadline {
            tick.tick().await;
            if let Ok(resp) = client_for_poll.get(&url).send().await {
                if let Ok(json) = resp.json::<serde_json::Value>().await {
                    if let Some(d) = json.get("queue_depth").and_then(|v| v.as_u64()) {
                        let prev = queue_max_poller.load(Ordering::Relaxed);
                        if d > prev {
                            queue_max_poller.fetch_max(d, Ordering::Relaxed);
                        }
                    }
                }
            }
        }
    });

    let started = Instant::now();
    for w in workers {
        let _ = w.await;
    }
    let _ = poller.await;
    let elapsed = started.elapsed();

    let h = hist.lock().await;
    let total_ok = success.load(Ordering::Relaxed);
    let total_err = errors.load(Ordering::Relaxed);
    let total = total_ok + total_err;

    let result = LoadtestResult {
        rate_target: args.rate,
        rate_achieved: total as f64 / elapsed.as_secs_f64(),
        duration_secs: elapsed.as_secs_f64(),
        total_requests: total,
        success_count: total_ok,
        error_count: total_err,
        p50_us: h.value_at_quantile(0.50),
        p95_us: h.value_at_quantile(0.95),
        p99_us: h.value_at_quantile(0.99),
        p999_us: h.value_at_quantile(0.999),
        p99_9_us: h.value_at_quantile(0.999),
        queue_depth_max: queue_max.load(Ordering::Relaxed),
    };
    Ok(result)
}

fn gate_on_baseline(result: &LoadtestResult, baseline_path: &PathBuf) -> Result<()> {
    if !baseline_path.exists() {
        // No baseline yet — write one and exit ok.
        let json = serde_json::to_string_pretty(result).context("failed to serialize baseline")?;
        std::fs::write(baseline_path, json)
            .with_context(|| format!("failed to write baseline at {}", baseline_path.display()))?;
        println!(
            "no baseline at {} — wrote current result as new baseline",
            baseline_path.display()
        );
        return Ok(());
    }

    let body = std::fs::read_to_string(baseline_path)
        .with_context(|| format!("failed to read baseline at {}", baseline_path.display()))?;
    let baseline: LoadtestResult =
        serde_json::from_str(&body).context("failed to parse baseline JSON")?;

    let p99_ratio = result.p99_us as f64 / baseline.p99_us.max(1) as f64;
    println!(
        "\n=== baseline gate ===\nbaseline p99: {} µs\ncurrent  p99: {} µs\nratio:        {:.3}x",
        baseline.p99_us, result.p99_us, p99_ratio,
    );
    if p99_ratio > 1.20 {
        bail!("p99 regression: {:.1}% over baseline (>20% threshold)", (p99_ratio - 1.0) * 100.0);
    }
    println!("PASS: p99 within 20% of baseline.");
    Ok(())
}

/// Parse `--rate=<n>` / `--duration=<s>` / `--concurrency=<n>` /
/// `--target=<url>` / `--baseline=<path>` / `--api-key=<key>` from a
/// pre-split arg vector. Returns the parsed args or an error message.
pub fn parse_args(args: Vec<String>) -> Result<LoadtestArgs> {
    let mut rate: Option<u64> = None;
    let mut duration: Option<Duration> = None;
    let mut concurrency: usize = 64;
    let mut target: String = "http://127.0.0.1:8080".to_owned();
    let mut baseline: Option<PathBuf> = None;
    let mut api_key: Option<String> = None;
    let mut durable: bool = true;

    let mut iter = args.into_iter();
    while let Some(a) = iter.next() {
        let (key, val) = if let Some((k, v)) = a.split_once('=') {
            (k.to_owned(), Some(v.to_owned()))
        } else {
            (a, iter.next())
        };
        match key.as_str() {
            "--rate" => {
                rate = Some(
                    val.ok_or_else(|| anyhow!("--rate requires a value"))?
                        .parse()
                        .context("invalid --rate")?,
                );
            },
            "--duration" => {
                let v = val.ok_or_else(|| anyhow!("--duration requires a value"))?;
                duration = Some(parse_duration(&v)?);
            },
            "--concurrency" => {
                concurrency = val
                    .ok_or_else(|| anyhow!("--concurrency requires a value"))?
                    .parse()
                    .context("invalid --concurrency")?;
            },
            "--target" => {
                target = val.ok_or_else(|| anyhow!("--target requires a value"))?;
            },
            "--baseline" => {
                baseline =
                    Some(PathBuf::from(val.ok_or_else(|| anyhow!("--baseline requires a value"))?));
            },
            "--api-key" => {
                api_key = Some(val.ok_or_else(|| anyhow!("--api-key requires a value"))?);
            },
            "--no-durable" => {
                durable = false;
                // Push back the consumed value if any (--no-durable
                // takes none, but split_once may have grabbed one).
                if let Some(v) = val {
                    bail!("--no-durable takes no value (got `{v}`)");
                }
            },
            other => bail!("unknown loadtest arg: {other}"),
        }
    }

    Ok(LoadtestArgs {
        rate: rate.ok_or_else(|| anyhow!("--rate is required"))?,
        duration: duration.ok_or_else(|| anyhow!("--duration is required"))?,
        concurrency: concurrency.max(1),
        target,
        baseline,
        api_key,
        durable,
    })
}

fn parse_duration(s: &str) -> Result<Duration> {
    if let Some(num) = s.strip_suffix("ms") {
        return Ok(Duration::from_millis(num.parse().context("invalid --duration ms")?));
    }
    if let Some(num) = s.strip_suffix('s') {
        return Ok(Duration::from_secs(num.parse().context("invalid --duration s")?));
    }
    if let Some(num) = s.strip_suffix('m') {
        return Ok(Duration::from_secs(
            num.parse::<u64>().context("invalid --duration m")?.saturating_mul(60),
        ));
    }
    // Fallback: treat as seconds.
    Ok(Duration::from_secs(s.parse().context("invalid --duration (use Ns / Nm / Nms)")?))
}
