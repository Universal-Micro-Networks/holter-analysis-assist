//! Axum router assembly: health + analyze + static UI with body limit and timeout.

use crate::http::error::HttpError;
use crate::http::handlers::{static_ui_router, AnalyzeHandler, HealthHandler};
use crate::http::state::AppState;
use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{extract::Request, Router};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;

/// Build the HTTP service router (design: routes.rs / RouterIntegration).
///
/// Mounts console static delivery alongside `/health` and `/v1/analyze`.
/// Body-size limit and request timeout come only from [`AppState`] / HttpConfig
/// (no UI-specific relaxation). Static UI does not add license metering.
pub fn build_router(state: AppState) -> Router {
    let max_body = state.max_body_bytes();
    let timeout = state.request_timeout();

    Router::new()
        .route("/health", get(HealthHandler::get))
        .route("/v1/analyze", post(AnalyzeHandler::post))
        .merge(static_ui_router())
        .layer(DefaultBodyLimit::max(max_body))
        .layer(RequestBodyLimitLayer::new(max_body))
        // TimeoutLayer is inner; map_timeout (outer) rewrites empty 408 → JSON 504.
        .layer(TimeoutLayer::new(timeout))
        .layer(middleware::from_fn(map_timeout_to_http_error))
        .with_state(state)
}

/// Convert tower-http empty 408 into the stable JSON error envelope (504).
async fn map_timeout_to_http_error(req: Request, next: Next) -> Response {
    let res = next.run(req).await;
    if res.status() == StatusCode::REQUEST_TIMEOUT {
        return HttpError::request_timeout("request_timeout_secs exceeded").into_response();
    }
    res
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::http::config::HttpConfig;
    use crate::license::{
        LicenseCheckResult, LicenseClient, LicenseError, LicenseGate, LicenseMeterResult,
        MockLicenseClient, MockOutcome, GLOBAL_TEST_LOCK,
    };
    use crate::model_source::ModelSource;
    use crate::phase2::ExecutionProviderKind;
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request as HttpRequest, StatusCode};
    use serde_json::Value;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tower::ServiceExt;

    struct CountingMeterClient {
        inner: MockLicenseClient,
        meter_calls: Arc<AtomicUsize>,
    }

    impl LicenseClient for CountingMeterClient {
        fn check_validity(&self) -> Result<LicenseCheckResult, LicenseError> {
            self.inner.check_validity()
        }

        fn authorize_and_meter(&self) -> Result<LicenseMeterResult, LicenseError> {
            self.meter_calls.fetch_add(1, Ordering::SeqCst);
            self.inner.authorize_and_meter()
        }
    }

    fn test_state() -> AppState {
        AppState::new(
            HttpConfig {
                bind: "127.0.0.1:0".into(),
                max_body_bytes: 1024 * 1024,
                request_timeout: Duration::from_secs(30),
                model_path: Some(PathBuf::from("/tmp/routes-test-missing.onnx")),
                provider: ExecutionProviderKind::Cpu,
            },
            ModelSource::Path(PathBuf::from("/tmp/routes-test-missing.onnx")),
        )
    }

    fn with_gate<T>(f: impl FnOnce(Arc<AtomicUsize>) -> T) -> T {
        let _guard = GLOBAL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        LicenseGate::clear_for_test();
        let meter_calls = Arc::new(AtomicUsize::new(0));
        let client = CountingMeterClient {
            inner: MockLicenseClient::new(
                MockOutcome::Success { message: None },
                MockOutcome::Success { message: None },
            ),
            meter_calls: Arc::clone(&meter_calls),
        };
        LicenseGate::install(LicenseGate::new(client)).expect("install");
        let out = f(Arc::clone(&meter_calls));
        LicenseGate::clear_for_test();
        out
    }

    fn block_on<F: std::future::Future>(fut: F) -> F::Output {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("runtime")
            .block_on(fut)
    }

    async fn oneshot_get(app: &axum::Router, uri: &str) -> axum::response::Response {
        app.clone()
            .oneshot(
                HttpRequest::builder()
                    .uri(uri)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("oneshot")
    }

    async fn oneshot_analyze(app: axum::Router) -> axum::response::Response {
        let boundary = "----RoutesUnitBoundary";
        let body = format!(
            "--{boundary}\r\n\
             Content-Disposition: form-data; name=\"ecl\"; filename=\"1234567890_20240101_0000_2359.ecl\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n\
             x\r\n\
             --{boundary}--\r\n"
        );
        app.oneshot(
            HttpRequest::builder()
                .method("POST")
                .uri("/v1/analyze")
                .header(
                    header::CONTENT_TYPE,
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(Body::from(body))
                .unwrap(),
        )
        .await
        .expect("analyze")
    }

    #[test]
    fn router_serves_health_and_analyze_paths() {
        with_gate(|meter_calls| {
            block_on(async {
                let app = build_router(test_state());
                let health = oneshot_get(&app, "/health").await;
                assert_eq!(health.status(), StatusCode::OK);
                assert_eq!(meter_calls.load(Ordering::SeqCst), 0);

                let analyze = oneshot_analyze(app).await;
                assert_ne!(analyze.status(), StatusCode::NOT_FOUND);
                assert_eq!(meter_calls.load(Ordering::SeqCst), 1);
                let bytes = to_bytes(analyze.into_body(), 1024 * 1024)
                    .await
                    .expect("body");
                let v: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
                assert!(
                    v.get("rows").is_none(),
                    "failure path must not leak analyze rows"
                );
            });
        });
    }

    /// Task 3.1 / design RouterIntegration: `/ui/`, `/health`, `/v1/analyze` coexist;
    /// static delivery does not meter; one analyze → one meter (req 1.3, 6.1, 6.2).
    #[test]
    fn build_router_serves_ui_health_analyze_with_single_meter() {
        with_gate(|meter_calls| {
            block_on(async {
                let app = build_router(test_state());

                let ui = oneshot_get(&app, "/ui/").await;
                assert_eq!(ui.status(), StatusCode::OK, "/ui/ must be mounted");
                let ui_ct = ui
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                assert!(
                    ui_ct.starts_with("text/html"),
                    "/ui/ Content-Type must be text/html, got {ui_ct}"
                );
                assert_eq!(
                    meter_calls.load(Ordering::SeqCst),
                    0,
                    "static UI must not meter"
                );

                let health = oneshot_get(&app, "/health").await;
                assert_eq!(health.status(), StatusCode::OK);
                let health_bytes = to_bytes(health.into_body(), 1024).await.expect("health body");
                let health_json: Value =
                    serde_json::from_slice(&health_bytes).expect("health json");
                assert_eq!(health_json["status"], "ok");
                assert_eq!(
                    meter_calls.load(Ordering::SeqCst),
                    0,
                    "health must not meter"
                );

                let root = oneshot_get(&app, "/").await;
                assert!(
                    root.status().is_redirection(),
                    "GET / should redirect to /ui/, got {}",
                    root.status()
                );

                let analyze = oneshot_analyze(app).await;
                assert_ne!(
                    analyze.status(),
                    StatusCode::NOT_FOUND,
                    "/v1/analyze must remain reachable"
                );
                assert_eq!(
                    meter_calls.load(Ordering::SeqCst),
                    1,
                    "exactly one meter per analyze; UI must not add metering"
                );
            });
        });
    }

    /// Task 4.1 / Requirements 1.1, 1.3, 2.2, 6.1–6.3:
    /// `/ui/` success, missing asset 404, health no meter, analyze meters once after UI,
    /// health/analyze regression on the full router.
    #[test]
    fn console_delivery_and_api_coexistence_validation() {
        with_gate(|meter_calls| {
            block_on(async {
                let app = build_router(test_state());

                // `/ui/` success (HTML)
                let ui = oneshot_get(&app, "/ui/").await;
                assert_eq!(ui.status(), StatusCode::OK, "/ui/ must succeed");
                let ui_ct = ui
                    .headers()
                    .get(header::CONTENT_TYPE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("");
                assert!(
                    ui_ct.starts_with("text/html"),
                    "/ui/ Content-Type must be text/html, got {ui_ct}"
                );
                assert_eq!(
                    meter_calls.load(Ordering::SeqCst),
                    0,
                    "static UI delivery must not meter"
                );

                // Missing asset → 404 (still no meter)
                let missing = oneshot_get(&app, "/ui/does-not-exist.xyz").await;
                assert_eq!(
                    missing.status(),
                    StatusCode::NOT_FOUND,
                    "missing console asset must be 404"
                );
                assert_eq!(
                    meter_calls.load(Ordering::SeqCst),
                    0,
                    "404 asset path must not meter"
                );

                // Health regression + no meter
                let health = oneshot_get(&app, "/health").await;
                assert_eq!(health.status(), StatusCode::OK, "/health regression");
                let health_bytes = to_bytes(health.into_body(), 1024).await.expect("health body");
                let health_json: Value =
                    serde_json::from_slice(&health_bytes).expect("health json");
                assert_eq!(health_json["status"], "ok");
                assert_eq!(
                    meter_calls.load(Ordering::SeqCst),
                    0,
                    "health must not meter"
                );

                // After UI (+ health), one analyze → exactly one meter
                let analyze = oneshot_analyze(app).await;
                assert_ne!(
                    analyze.status(),
                    StatusCode::NOT_FOUND,
                    "/v1/analyze regression: must remain reachable"
                );
                assert_eq!(
                    meter_calls.load(Ordering::SeqCst),
                    1,
                    "exactly one meter after UI + health + one analyze"
                );
            });
        });
    }
}
