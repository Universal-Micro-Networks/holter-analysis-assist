//! Axum router assembly: health + analyze with body limit and timeout.

use crate::http::error::HttpError;
use crate::http::handlers::{AnalyzeHandler, HealthHandler};
use crate::http::state::AppState;
use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{extract::Request, Router};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::timeout::TimeoutLayer;

/// Build the HTTP service router (design: routes.rs).
///
/// Applies configured body-size limit and end-to-end request timeout.
pub fn build_router(state: AppState) -> Router {
    let max_body = state.max_body_bytes();
    let timeout = state.request_timeout();

    Router::new()
        .route("/health", get(HealthHandler::get))
        .route("/v1/analyze", post(AnalyzeHandler::post))
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

    #[test]
    fn router_serves_health_and_analyze_paths() {
        with_gate(|meter_calls| {
            block_on(async {
                let app = build_router(test_state());
                let health = app
                    .clone()
                    .oneshot(
                        HttpRequest::builder()
                            .uri("/health")
                            .body(Body::empty())
                            .unwrap(),
                    )
                    .await
                    .expect("health");
                assert_eq!(health.status(), StatusCode::OK);
                assert_eq!(meter_calls.load(Ordering::SeqCst), 0);

                let boundary = "----RoutesUnitBoundary";
                let body = format!(
                    "--{boundary}\r\n\
                     Content-Disposition: form-data; name=\"ecl\"; filename=\"1234567890_20240101_0000_2359.ecl\"\r\n\
                     Content-Type: application/octet-stream\r\n\r\n\
                     x\r\n\
                     --{boundary}--\r\n"
                );
                let analyze = app
                    .oneshot(
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
                    .expect("analyze");
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
}
