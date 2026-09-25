//! http-api validation task 5.1 — unit-level contract checks via the public HTTP API.
//!
//! Covers: defaults 512 MiB / 1800s, missing required `bind`, error-code mapping,
//! and health success body. License non-metering for `/health` is asserted in
//! `src/http/handlers/health.rs` (counting mock + process listen tests).

use axum::body::{to_bytes, Body};
use axum::http::{Request, StatusCode};
use axum::routing::get;
use axum::{Json, Router};
use holter_analysis_assist::http::config::{
    HttpConfig, HttpConfigError, DEFAULT_MAX_BODY_BYTES, DEFAULT_REQUEST_TIMEOUT_SECS,
};
use holter_analysis_assist::http::{HealthHandler, HttpError};
use serde_json::Value;
use std::io::Write;
use std::time::Duration;
use tempfile::NamedTempFile;
use tower::ServiceExt;

fn write_ini(body: &str) -> NamedTempFile {
    let mut f = NamedTempFile::new().expect("temp ini");
    f.write_all(body.as_bytes()).expect("write ini");
    f
}

#[test]
fn defaults_are_512_mib_and_1800_seconds() {
    assert_eq!(DEFAULT_MAX_BODY_BYTES, 512 * 1024 * 1024);
    assert_eq!(DEFAULT_MAX_BODY_BYTES, 536_870_912);
    assert_eq!(DEFAULT_REQUEST_TIMEOUT_SECS, 1800);

    let ini = write_ini(
        r#"[http]
bind=127.0.0.1:18080
"#,
    );
    let cfg = HttpConfig::load_from_path(ini.path()).expect("minimal ini");
    assert_eq!(cfg.max_body_bytes, DEFAULT_MAX_BODY_BYTES);
    assert_eq!(
        cfg.request_timeout,
        Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS)
    );
}

#[test]
fn missing_required_bind_is_config_error() {
    let ini = write_ini(
        r#"[http]
max_body_bytes=1024
"#,
    );
    let err = HttpConfig::load_from_path(ini.path()).expect_err("missing bind");
    assert!(matches!(err, HttpConfigError::Config(_)));
    let msg = err.to_string();
    assert!(
        msg.contains("bind") || msg.to_lowercase().contains("missing"),
        "fail-closed message should identify missing bind: {msg}"
    );
}

#[test]
fn error_categories_map_to_stable_codes_and_status() {
    let cases: &[(HttpError, StatusCode, &str)] = &[
        (
            HttpError::invalid_input("bad ecl"),
            StatusCode::BAD_REQUEST,
            "invalid_input",
        ),
        (
            HttpError::payload_too_large("too big"),
            StatusCode::PAYLOAD_TOO_LARGE,
            "payload_too_large",
        ),
        (
            HttpError::license_inference_denied("quota"),
            StatusCode::FORBIDDEN,
            "license_inference_denied",
        ),
        (
            HttpError::request_timeout("slow"),
            StatusCode::GATEWAY_TIMEOUT,
            "request_timeout",
        ),
        (
            HttpError::internal("boom"),
            StatusCode::INTERNAL_SERVER_ERROR,
            "internal_error",
        ),
    ];
    for (err, status, code) in cases {
        assert_eq!(err.status_code(), *status, "code={code}");
        assert_eq!(err.error_code(), *code);
        let body = err.to_error_body();
        assert_eq!(body.error.code, *code);
        assert!(!body.error.message.is_empty());
    }
}

#[tokio::test]
async fn health_returns_ok_without_analyze_path() {
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
        .expect("oneshot");
    assert_eq!(response.status(), StatusCode::OK);
    let bytes = to_bytes(response.into_body(), 1024).await.expect("body");
    let v: Value = serde_json::from_slice(&bytes).expect("json");
    assert_eq!(v["status"], "ok");
}
