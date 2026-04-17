//! `cargo xtask <subcommand>` — project automation.

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
        Some("--help") | Some("-h") | None => {
            println!("xtask — Keplor project automation");
            println!();
            println!("USAGE: cargo xtask <subcommand>");
            println!();
            println!("SUBCOMMANDS:");
            println!("  refresh-catalog   download + pin LiteLLM pricing catalogue");
            println!("  size-audit        report release-binary size vs. phase gate");
            ExitCode::SUCCESS
        },
        Some(unknown) => {
            eprintln!("xtask: unknown subcommand `{unknown}`");
            ExitCode::FAILURE
        },
    }
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
