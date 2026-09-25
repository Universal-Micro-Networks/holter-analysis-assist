//! HTTP server binary entry (`holter-http-api`).
//!
//! Startup order (design HttpStartup):
//! load ini → LicenseGate::install → ensure_startup_licensed → AppState → bind.
//! Failures exit non-zero without listening.
//!
//! License install uses the blocking reqwest client and therefore runs *before*
//! the Tokio multi-thread runtime is entered (same fail-closed contract as CLI).

use clap::Parser;
use holter_analysis_assist::http::{
    install_and_ensure_startup_licensed, serve, HttpConfig,
};
use std::path::PathBuf;
use std::process::ExitCode;

/// Default relative path when neither `--config` nor `HOLTER_HTTP_INI` is set.
/// The file must contain both `[http]` and `[license]` (merged), or operators
/// may point at a merged copy of `config/http.ini.example` + license keys.
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

    let http_config = match HttpConfig::load_from_path(&args.config) {
        Ok(cfg) => cfg,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };

    // Fail-closed license gate before any listen (sync; no Tokio yet).
    if let Err(err) = install_and_ensure_startup_licensed(&args.config) {
        eprintln!("error: {err}");
        return ExitCode::FAILURE;
    }

    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(err) => {
            eprintln!("error: failed to start async runtime: {err}");
            return ExitCode::FAILURE;
        }
    };

    match runtime.block_on(serve(http_config)) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}
