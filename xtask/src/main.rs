//! `cargo xtask <subcommand>` — project automation.
//!
//! Phase-0 stub: subcommands (`refresh-catalog`, `size-audit`) are added
//! in later phases.

use std::process::ExitCode;

fn main() -> ExitCode {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("--help") | Some("-h") | None => {
            println!("xtask — Keplor project automation");
            println!();
            println!("USAGE: cargo xtask <subcommand>");
            println!();
            println!("SUBCOMMANDS (added in later phases):");
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
