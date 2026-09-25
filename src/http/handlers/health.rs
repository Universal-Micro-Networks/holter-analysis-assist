//! GET `/health` — process accept readiness without license metering.
//!
//! Requirements 5.1–5.3. Analyze route wiring is a later task; this module
//! owns only the health response and its non-metering invariant.

use axum::Json;
use serde::Serialize;

/// Health-check handler (design: HealthHandler).
pub struct HealthHandler;

/// Success body: `{ "status": "ok" }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct HealthBody {
    pub status: &'static str,
}

impl HealthHandler {
    /// Pure success body used by the route handler (no LicenseGate / analyze).
    pub fn body() -> HealthBody {
        HealthBody { status: "ok" }
    }

    /// Axum handler: indicate the process can accept requests.
    ///
    /// Must not call LicenseGate meter / `ensure_inference_allowed` / analyze.
    pub async fn get() -> Json<HealthBody> {
        Json(Self::body())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::license::{
        LicenseCheckResult, LicenseClient, LicenseError, LicenseGate, LicenseMeterResult,
        MockLicenseClient, MockOutcome, GLOBAL_TEST_LOCK,
    };
    use axum::body::{to_bytes, Body};
    use axum::http::{Request, StatusCode};
    use axum::routing::get;
    use axum::Router;
    use serde_json::Value;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tower::ServiceExt;

    /// Counts check vs meter to prove health never meters.
    struct CountingClient {
        inner: MockLicenseClient,
        check_calls: Arc<AtomicUsize>,
        meter_calls: Arc<AtomicUsize>,
    }

    impl LicenseClient for CountingClient {
        fn check_validity(&self) -> Result<LicenseCheckResult, LicenseError> {
            self.check_calls.fetch_add(1, Ordering::SeqCst);
            self.inner.check_validity()
        }

        fn authorize_and_meter(&self) -> Result<LicenseMeterResult, LicenseError> {
            self.meter_calls.fetch_add(1, Ordering::SeqCst);
            self.inner.authorize_and_meter()
        }
    }

    #[tokio::test]
    async fn health_handler_returns_status_ok() {
        let Json(body) = HealthHandler::get().await;
        assert_eq!(body.status, "ok");
    }

    #[tokio::test]
    async fn get_health_route_returns_200_with_ok_json() {
        let app = Router::new().route("/health", get(HealthHandler::get));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("oneshot");
        assert_eq!(response.status(), StatusCode::OK);
        let bytes = to_bytes(response.into_body(), 1024).await.expect("body");
        let v: Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(v["status"], "ok");
    }

    #[tokio::test]
    async fn health_path_is_distinct_from_analyze() {
        // Requirement 5.3: operational path ≠ analyze (`POST /v1/analyze`).
        const HEALTH_PATH: &str = "/health";
        const ANALYZE_PATH: &str = "/v1/analyze";
        assert_ne!(HEALTH_PATH, ANALYZE_PATH);

        let app = Router::new().route(HEALTH_PATH, get(HealthHandler::get));
        let ok = app
            .clone()
            .oneshot(
                Request::builder()
                    .uri(HEALTH_PATH)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("health");
        assert_eq!(ok.status(), StatusCode::OK);

        let miss = app
            .oneshot(
                Request::builder()
                    .uri(ANALYZE_PATH)
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("analyze miss");
        assert_eq!(
            miss.status(),
            StatusCode::NOT_FOUND,
            "analyze path must not be served by health-only router"
        );
    }

    #[tokio::test]
    async fn health_does_not_call_license_gate_meter_or_ensure_inference() {
        let check_calls = Arc::new(AtomicUsize::new(0));
        let meter_calls = Arc::new(AtomicUsize::new(0));

        // Install under GLOBAL_TEST_LOCK; do not hold the lock across await.
        {
            let _guard = GLOBAL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            LicenseGate::clear_for_test();
            let client = CountingClient {
                inner: MockLicenseClient::new(
                    MockOutcome::Success { message: None },
                    MockOutcome::Success { message: None },
                ),
                check_calls: Arc::clone(&check_calls),
                meter_calls: Arc::clone(&meter_calls),
            };
            LicenseGate::install(LicenseGate::new(client)).expect("install");
        }

        let Json(body) = HealthHandler::get().await;
        assert_eq!(body.status, "ok");

        let app = Router::new().route("/health", get(HealthHandler::get));
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/health")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .expect("health oneshot");
        assert_eq!(response.status(), StatusCode::OK);

        {
            let _guard = GLOBAL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            assert_eq!(
                meter_calls.load(Ordering::SeqCst),
                0,
                "health must not call authorize_and_meter / ensure_inference_allowed"
            );
            assert_eq!(
                check_calls.load(Ordering::SeqCst),
                0,
                "health must not call check_validity either"
            );
            LicenseGate::clear_for_test();
        }
    }
}
