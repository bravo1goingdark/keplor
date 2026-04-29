//! `cargo xtask <subcommand>` — project automation.

mod loadtest;

use std::path::Path;
use std::process::{Command, ExitCode};

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("refresh-catalog") => {
            if let Err(e) = refresh_catalog() {
                eprintln!("xtask: refresh-catalog failed: {e:#}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        },
        Some("mem-audit") => {
            if let Err(e) = mem_audit() {
                eprintln!("xtask: mem-audit failed: {e:#}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        },
        Some("loadtest") => {
            let rest: Vec<String> = args.collect();
            let parsed = match loadtest::parse_args(rest) {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("xtask: loadtest: {e:#}");
                    eprintln!();
                    print_loadtest_help();
                    return ExitCode::FAILURE;
                },
            };
            if let Err(e) = loadtest::run(parsed) {
                eprintln!("xtask: loadtest failed: {e:#}");
                return ExitCode::FAILURE;
            }
            ExitCode::SUCCESS
        },
        Some("--help") | Some("-h") | None => {
            println!("xtask — Keplor project automation");
            println!();
            println!("USAGE: cargo xtask <subcommand>");
            println!();
            println!("SUBCOMMANDS:");
            println!("  refresh-catalog   download + pin LiteLLM pricing catalogue");
            println!("  size-audit        report release-binary size vs. phase gate");
            println!("  mem-audit         ingest events and check RSS stays under 30 MB");
            println!(
                "  loadtest          drive sustained POST /v1/events traffic, report p50/p95/p99"
            );
            ExitCode::SUCCESS
        },
        Some(unknown) => {
            eprintln!("xtask: unknown subcommand `{unknown}`");
            ExitCode::FAILURE
        },
    }
}

fn print_loadtest_help() {
    eprintln!("loadtest — drive sustained POST /v1/events traffic.");
    eprintln!();
    eprintln!("USAGE: cargo xtask loadtest --rate <N> --duration <D> [options]");
    eprintln!();
    eprintln!("REQUIRED:");
    eprintln!("  --rate <N>              aggregate requests per second");
    eprintln!("  --duration <D>          how long to sustain (e.g. 30s, 2m, 500ms)");
    eprintln!();
    eprintln!("OPTIONAL:");
    eprintln!("  --concurrency <N>       worker tasks (default 64)");
    eprintln!("  --target <URL>          base URL (default http://127.0.0.1:8080)");
    eprintln!("  --baseline <PATH>       compare p99 against saved JSON; >20% regression = exit 1");
    eprintln!("  --api-key <KEY>         Authorization: Bearer <KEY>");
}

/// Download the latest LiteLLM pricing catalogue, update the bundled
/// snapshot and version constants, then run the test suite.
fn refresh_catalog() -> anyhow::Result<()> {
    let workspace_root = workspace_root()?;
    let catalog_path =
        workspace_root.join("crates/keplor-pricing/data/model_prices_and_context_window.json");
    let catalog_rs = workspace_root.join("crates/keplor-pricing/src/catalog.rs");

    let url =
        "https://raw.githubusercontent.com/BerriAI/litellm/main/litellm/model_prices_and_context_window_backup.json";

    // 1. Fetch latest commit SHA touching this file.
    println!("Fetching latest commit SHA...");
    let sha_output = Command::new("curl")
        .args([
            "-fsSL",
            "https://api.github.com/repos/BerriAI/litellm/commits?path=litellm/model_prices_and_context_window_backup.json&sha=main&per_page=1",
        ])
        .output()?;
    anyhow::ensure!(sha_output.status.success(), "failed to query GitHub API");
    let sha_json: serde_json::Value = serde_json::from_slice(&sha_output.stdout)?;
    let sha =
        sha_json[0]["sha"].as_str().ok_or_else(|| anyhow::anyhow!("no SHA in GitHub response"))?;
    println!("  SHA: {sha}");

    // 2. Download the catalogue.
    println!("Downloading catalogue...");
    let dl = Command::new("curl").args(["-fsSL", "-o"]).arg(&catalog_path).arg(url).status()?;
    anyhow::ensure!(dl.success(), "curl download failed");
    let size = std::fs::metadata(&catalog_path)?.len();
    println!("  Downloaded {} bytes → {}", size, catalog_path.display());

    // 3. Update version constants in catalog.rs.
    let today = chrono_lite_today()?;
    println!("Updating catalog.rs constants (SHA={sha}, date={today})...");
    let src = std::fs::read_to_string(&catalog_rs)?;
    let updated = update_const(&src, "PRICING_CATALOG_VERSION", sha)?;
    let updated = update_const(&updated, "PRICING_CATALOG_DATE", &today)?;
    std::fs::write(&catalog_rs, updated)?;

    // 4. Run the test suite.
    println!("Running tests...");
    let test = Command::new("cargo").args(["test", "-p", "keplor-pricing"]).status()?;
    anyhow::ensure!(test.success(), "tests failed after catalog refresh");

    println!("Done. Catalogue pinned at {sha} ({today}).");
    Ok(())
}

/// Replace the value of `pub const NAME: &str = "...";` in source text.
fn update_const(src: &str, name: &str, value: &str) -> anyhow::Result<String> {
    let pattern = format!("pub const {name}: &str = \"");
    let start = src
        .find(&pattern)
        .ok_or_else(|| anyhow::anyhow!("constant {name} not found in catalog.rs"))?;
    let after_quote = start + pattern.len();
    let end_quote = src[after_quote..]
        .find('"')
        .ok_or_else(|| anyhow::anyhow!("unterminated string for {name}"))?
        + after_quote;
    let mut result = String::with_capacity(src.len());
    result.push_str(&src[..after_quote]);
    result.push_str(value);
    result.push_str(&src[end_quote..]);
    Ok(result)
}

fn workspace_root() -> anyhow::Result<std::path::PathBuf> {
    let output = Command::new("cargo")
        .args(["locate-project", "--workspace", "--message-format=plain"])
        .output()?;
    anyhow::ensure!(output.status.success(), "cargo locate-project failed");
    let path = String::from_utf8(output.stdout)?.trim().to_string();
    Ok(Path::new(&path)
        .parent()
        .ok_or_else(|| anyhow::anyhow!("no parent for Cargo.toml path"))?
        .to_path_buf())
}

fn chrono_lite_today() -> anyhow::Result<String> {
    let output = Command::new("date").arg("+%Y-%m-%d").output()?;
    anyhow::ensure!(output.status.success(), "`date` command failed");
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

/// Build the server, start it, ingest events, check RSS stays under 30 MB.
fn mem_audit() -> anyhow::Result<()> {
    let workspace = workspace_root()?;
    let config = workspace.join("tests/fixtures/bench-config.toml");
    let payload = workspace.join("tests/fixtures/load/single-event.json");

    anyhow::ensure!(config.exists(), "bench config not found at {}", config.display());
    anyhow::ensure!(payload.exists(), "load payload not found at {}", payload.display());

    // Build with bench profile.
    println!("Building keplor (profile=bench)...");
    let build =
        Command::new("cargo").args(["build", "--profile", "bench", "-p", "keplor-cli"]).status()?;
    anyhow::ensure!(build.success(), "build failed");

    // The built-in bench profile outputs to target/release/ (not target/bench/).
    let binary = workspace.join("target/release/keplor");
    anyhow::ensure!(binary.exists(), "binary not found at {}", binary.display());

    // Create a tmpdir for the DB.
    let tmp = std::env::temp_dir().join("keplor-mem-audit");
    let _ = std::fs::remove_dir_all(&tmp);
    std::fs::create_dir_all(&tmp)?;
    let db_path = tmp.join("audit.db");

    // Start the server.
    println!("Starting server (db={})...", db_path.display());
    let mut server = Command::new(&binary)
        .args(["run", "--config"])
        .arg(&config)
        .env("KEPLOR_STORAGE_DB_PATH", &db_path)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let pid = server.id();

    // Wait for server to be healthy.
    let mut healthy = false;
    for _ in 0..50 {
        std::thread::sleep(std::time::Duration::from_millis(100));
        if Command::new("curl")
            .args(["-sf", "http://127.0.0.1:8080/health"])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
        {
            healthy = true;
            break;
        }
    }

    if !healthy {
        let _ = server.kill();
        anyhow::bail!("server did not become healthy within 5s");
    }
    println!("Server ready (pid={pid})");

    // Ingest 5000 events.
    let event_count = 5000;
    println!("Ingesting {event_count} events...");
    let payload_data = std::fs::read_to_string(&payload)?;
    for i in 0..event_count {
        let result = Command::new("curl")
            .args([
                "-sf",
                "-X",
                "POST",
                "-H",
                "Content-Type: application/json",
                "-d",
                &payload_data,
                "http://127.0.0.1:8080/v1/events",
            ])
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status();
        if i % 1000 == 0 && i > 0 {
            println!("  ...{i}/{event_count}");
        }
        if let Ok(status) = result {
            if !status.success() {
                eprintln!("  warning: request {i} failed");
            }
        }
    }
    println!("Ingestion complete.");

    // Give the batch writer time to flush.
    std::thread::sleep(std::time::Duration::from_millis(500));

    // Check RSS.
    let rss_kb = read_rss_kb(pid);
    let _ = server.kill();
    let _ = server.wait();
    let _ = std::fs::remove_dir_all(&tmp);

    match rss_kb {
        Some(kb) => {
            let mb = kb / 1024;
            println!("VmRSS after {event_count} events: {mb} MB ({kb} KB)");
            if mb > 30 {
                anyhow::bail!("FAIL: RSS {mb} MB exceeds 30 MB target");
            }
            println!("PASS: RSS {mb} MB is within 30 MB target");
        },
        None => {
            println!("WARNING: could not read /proc/{pid}/status — RSS check skipped");
        },
    }

    Ok(())
}

/// Read VmRSS from /proc/<pid>/status, returning the value in KB.
fn read_rss_kb(pid: u32) -> Option<u64> {
    use std::io::BufRead;
    let path = format!("/proc/{pid}/status");
    let file = std::fs::File::open(path).ok()?;
    let reader = std::io::BufReader::new(file);
    for line in reader.lines() {
        let line = line.ok()?;
        if line.starts_with("VmRSS:") {
            let kb_str = line.split_whitespace().nth(1)?;
            return kb_str.parse().ok();
        }
    }
    None
}
