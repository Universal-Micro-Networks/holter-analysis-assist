//! HTTP server binary entry (`holter-http-api`).
//!
//! Startup order (design HttpStartup):
//! load ini → LicenseGate::install → ensure_startup_licensed → AppState → bind.
//! Failures exit non-zero without listening.
//!
//! Prefer `holter-analysis-assist serve-http` when Device Guard blocks this binary.

use clap::Parser;
use holter_analysis_assist::http::run_blocking;
use std::path::PathBuf;
use std::process::ExitCode;

/// Default relative path when neither `--config` nor `HOLTER_HTTP_INI` is set.
const DEFAULT_HTTP_INI: &str = "config/http.ini";

#[derive(Parser, Debug)]
#[command(
    name = "holter-http-api",
    version,
    about = "Holter ECG arrhythmia assist — HTTP API (library-first adapter)"
)]
struct Args {
    /// Path to ini containing `[http]` and `[license]` sections.
    /// Overrides `HOLTER_HTTP_INI`; default `config/http.ini`.
    #[arg(
        long = "config",
        env = "HOLTER_HTTP_INI",
        default_value = DEFAULT_HTTP_INI,
        value_name = "PATH"
    )]
    config: PathBuf,
}

fn main() -> ExitCode {
    let args = Args::parse();
    match run_blocking(&args.config) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
