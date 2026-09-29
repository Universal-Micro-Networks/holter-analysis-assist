//! HTTP process startup: config → LicenseGate install → listen (fail-closed).

use crate::http::config::{HttpConfig, HttpConfigError};
use crate::http::error::HttpError;
use crate::http::routes::build_router;
use crate::http::state::AppState;
use crate::license::{
    LicenseConfig, LicenseError, LicenseFailure, LicenseGate, ReqwestLicenseClient,
};
use crate::phase2::InferError;
use std::path::Path;
use std::time::Duration;
use thiserror::Error;
use tokio::net::TcpListener;

/// How often a failed startup validity check is retried while serving.
pub const LICENSE_RECHECK_INTERVAL: Duration = Duration::from_secs(30);

/// Failures during HTTP process startup (before or while listening).
#[derive(Debug, Error)]
pub enum StartupError {
    #[error("http config error: {0}")]
    HttpConfig(#[from] HttpConfigError),
    #[error("{0}")]
    License(#[from] LicenseError),
    #[error("model load or warm-up failed: {0}")]
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

/// HTTP variant of [`install_and_ensure_startup_licensed`]: a failed validity
/// check is logged and the process keeps starting so the console can show it.
/// Analysis stays denied until a recheck passes; config errors still abort.
pub fn install_license_gate_for_http(ini_path: &Path) -> Result<(), LicenseError> {
    match install_and_ensure_startup_licensed(ini_path) {
        Err(LicenseError::StartupFailed(failure)) => {
            eprintln!(
                "{}; serving with analysis disabled, retrying every {}s",
                license_failure_log_line("startup check failed", &failure),
                LICENSE_RECHECK_INTERVAL.as_secs()
            );
            Ok(())
        }
        other => other,
    }
}

/// Stderr line for a failed validity check, with the message redacted like API errors.
fn license_failure_log_line(event: &str, failure: &LicenseFailure) -> String {
    let err = HttpError::from(LicenseError::InferenceDenied(failure.clone()));
    format!(
        "license: {event} code={} message={}",
        err.error_code(),
        err.message()
    )
}

/// Retry the startup validity check while it has not passed.
async fn recheck_license_until_verified(interval: Duration) {
    let mut ticker = tokio::time::interval(interval);
    ticker.tick().await;
    loop {
        ticker.tick().await;
        let Some(gate) = LicenseGate::try_global() else {
            return;
        };
        if gate.startup_failure().is_none() {
            return;
        }
        match tokio::task::spawn_blocking(move || gate.ensure_startup_licensed()).await {
            Ok(Ok(())) => {
                eprintln!("license: recheck passed; analysis enabled");
                return;
            }
            Ok(Err(LicenseError::StartupFailed(failure))) => {
                eprintln!("{}", license_failure_log_line("recheck failed", &failure));
            }
            Ok(Err(err)) => eprintln!("license: recheck failed: {err}"),
            Err(join) => eprintln!("license: recheck task failed: {join}"),
        }
    }
}

/// Sync entry for `holter-http-api` and CLI `serve-http`.
///
/// Installs the license gate (blocking), loads the resident model, then serves.
/// Must be called **outside** an existing Tokio runtime.
pub fn run_blocking(config_path: &Path) -> Result<(), StartupError> {
    let http_config = HttpConfig::load_from_path(config_path)?;
    install_license_gate_for_http(config_path)?;
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
    tokio::task::spawn_blocking(move || install_license_gate_for_http(&path))
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

    if LicenseGate::try_global().is_some_and(|gate| gate.startup_failure().is_some()) {
        tokio::spawn(recheck_license_until_verified(LICENSE_RECHECK_INTERVAL));
    }

    axum::serve(listener, app)
        .await
        .map_err(StartupError::Serve)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference_options::{BatchSize, CudaTuning};
    use crate::license::{
        LicenseCheckResult, LicenseClient, LicenseFailureReason, LicenseMeterResult,
        GLOBAL_TEST_LOCK,
    };
    use crate::phase2::ExecutionProviderKind;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    struct FlakyVerifyClient {
        reachable: Arc<AtomicBool>,
    }

    impl LicenseClient for FlakyVerifyClient {
        fn check_validity(&self) -> Result<LicenseCheckResult, LicenseError> {
            if self.reachable.load(Ordering::SeqCst) {
                Ok(LicenseCheckResult {
                    allowed: true,
                    ..Default::default()
                })
            } else {
                Err(LicenseError::StartupFailed(LicenseFailure::new(
                    LicenseFailureReason::TemporaryFailure,
                    "license server unreachable",
                )))
            }
        }

        fn authorize_and_meter(&self) -> Result<LicenseMeterResult, LicenseError> {
            Ok(LicenseMeterResult {
                allowed: true,
                ..Default::default()
            })
        }
    }

    #[test]
    fn recheck_clears_startup_failure_once_license_server_is_back() {
        let _guard = GLOBAL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        LicenseGate::clear_for_test();
        let reachable = Arc::new(AtomicBool::new(false));
        LicenseGate::install(LicenseGate::new(FlakyVerifyClient {
            reachable: Arc::clone(&reachable),
        }))
        .expect("install");
        let gate = LicenseGate::global();
        assert!(gate.ensure_startup_licensed().is_err());

        let runtime = tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("runtime");
        runtime.block_on(async {
            let task = tokio::spawn(recheck_license_until_verified(Duration::from_millis(20)));
            tokio::time::sleep(Duration::from_millis(80)).await;
            assert!(
                gate.startup_failure().is_some(),
                "failed rechecks keep analysis disabled"
            );
            reachable.store(true, Ordering::SeqCst);
            tokio::time::timeout(Duration::from_secs(5), task)
                .await
                .expect("recheck loop stops after a passing check")
                .expect("recheck task");
        });
        assert_eq!(gate.startup_failure(), None);
        gate.ensure_inference_allowed()
            .expect("analysis is enabled after recovery");
        LicenseGate::clear_for_test();
    }

    #[test]
    fn failure_log_line_uses_api_error_code_and_redacts_key() {
        let line = license_failure_log_line(
            "startup check failed",
            &LicenseFailure::new(
                LicenseFailureReason::LicenseInvalid,
                "unknown key lk_0123456789abcdef0123456789abcdef",
            ),
        );
        assert_eq!(
            line,
            "license: startup check failed code=license_inference_denied \
             message=license_invalid: unknown key lk_[REDACTED]"
        );
    }

    #[tokio::test]
    async fn serve_with_missing_model_fails_before_listening() {
        let bind = {
            let probe = std::net::TcpListener::bind("127.0.0.1:0").expect("probe bind");
            probe.local_addr().expect("probe addr").to_string()
        };
        let config = HttpConfig {
            bind: bind.clone(),
            max_body_bytes: 1024,
            request_timeout: Duration::from_secs(30),
            model_path: Some(PathBuf::from("/tmp/http-startup-missing-model.onnx")),
            provider: ExecutionProviderKind::Cpu,
            batch_size: BatchSize::default(),
            cuda: CudaTuning::default(),
        };

        let err = serve(config).await.expect_err("missing model must fail");
        assert!(matches!(err, StartupError::Model(_)), "{err}");
        let message = err.to_string();
        assert!(message.contains("warm-up"), "{message}");
        assert!(
            message.contains("http-startup-missing-model.onnx"),
            "failure reason must name the model: {message}"
        );
        assert!(
            std::net::TcpStream::connect(&bind).is_err(),
            "must not listen on {bind} after a model failure"
        );
    }
}
