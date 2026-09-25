//! HTTP process startup: config → LicenseGate install → listen (fail-closed).

use crate::http::config::{HttpConfig, HttpConfigError};
use crate::http::routes::build_router;
use crate::http::state::AppState;
use crate::license::{LicenseConfig, LicenseError, LicenseGate, ReqwestLicenseClient};
use crate::phase2::InferError;
use std::path::Path;
use thiserror::Error;
use tokio::net::TcpListener;

/// Failures during HTTP process startup (before or while listening).
#[derive(Debug, Error)]
pub enum StartupError {
    #[error("http config error: {0}")]
    HttpConfig(#[from] HttpConfigError),
    #[error("{0}")]
    License(#[from] LicenseError),
    #[error("model load failed: {0}")]
    Model(#[from] InferError),
    #[error("bind failed on {addr}: {source}")]
    Bind {
        addr: String,
        #[source]
        source: std::io::Error,
    },
    #[error("serve failed: {0}")]
    Serve(#[source] std::io::Error),
}

/// Load `[license]` from `ini_path`, install the process-wide gate, and run
/// [`LicenseGate::ensure_startup_licensed`].
///
/// Must complete successfully before any listen/bind.
pub fn install_and_ensure_startup_licensed(ini_path: &Path) -> Result<(), LicenseError> {
    let config = LicenseConfig::load_from_path(ini_path)?;
    let client = ReqwestLicenseClient::new(config)?;
    LicenseGate::install(LicenseGate::new(client))?;
    LicenseGate::global().ensure_startup_licensed()
}

/// Sync entry for `holter-http-api` and CLI `serve-http`.
///
/// Installs the license gate (blocking), loads the resident model, then serves.
/// Must be called **outside** an existing Tokio runtime.
pub fn run_blocking(config_path: &Path) -> Result<(), StartupError> {
    let http_config = HttpConfig::load_from_path(config_path)?;
    install_and_ensure_startup_licensed(config_path)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(StartupError::Serve)?;
    runtime.block_on(serve(http_config))
}

/// Full startup sequence (design HttpStartup):
/// load `[http]` → install gate → `ensure_startup_licensed` → AppState → bind/serve.
///
/// On any failure before bind, the port is never opened.
///
/// License HTTP uses a blocking client; it runs on `spawn_blocking` so it is not
/// nested inside the Tokio runtime (avoids "drop a runtime in async context").
pub async fn run(config_path: &Path) -> Result<(), StartupError> {
    let http_config = HttpConfig::load_from_path(config_path)?;
    let path = config_path.to_path_buf();
    tokio::task::spawn_blocking(move || install_and_ensure_startup_licensed(&path))
        .await
        .map_err(|e| {
            StartupError::Serve(std::io::Error::other(format!(
                "license startup task join failed: {e}"
            )))
        })??;
    serve(http_config).await
}

/// Bind and serve after license gate has already succeeded.
///
/// Loads the Phase-2 ONNX model once into [`AppState`] before listening.
pub async fn serve(http_config: HttpConfig) -> Result<(), StartupError> {
    let bind = http_config.bind.clone();
    let state = AppState::from_config(http_config)?;
    let app = build_router(state);

    let listener = TcpListener::bind(&bind)
        .await
        .map_err(|source| StartupError::Bind {
            addr: bind.clone(),
            source,
        })?;

    match listener.local_addr() {
        Ok(local) => eprintln!("holter-http-api: listening on http://{local}"),
        Err(_) => eprintln!("holter-http-api: listening on {bind}"),
    }

    axum::serve(listener, app)
        .await
        .map_err(StartupError::Serve)
}
