//! GET `/ui/` (and assets) — embedded console delivery without license metering.
//!
//! Design component: StaticUiHandler.
//! Mounted into [`crate::http::routes::build_router`] via [`static_ui_router`].

use crate::http::assets::EmbeddedConsoleAssets;
use axum::extract::Path;
use axum::http::{header, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::get;
use axum::Router;

/// Static console UI handler (design: StaticUiHandler).
pub struct StaticUiHandler;

impl StaticUiHandler {
    /// Serve `index.html` for `GET /ui` and `GET /ui/`.
    pub async fn index() -> Response {
        Self::asset_response("index.html")
    }

    /// Serve an embedded asset under `/ui/*path` (404 when missing / unsafe).
    pub async fn asset(Path(path): Path<String>) -> Response {
        Self::asset_response(&path)
    }

    /// Optional convenience: `GET /` → `/ui/`.
    pub async fn redirect_root() -> Redirect {
        Redirect::temporary("/ui/")
    }

    fn asset_response(path: &str) -> Response {
        let Some(key) = normalize_asset_key(path) else {
            return StatusCode::NOT_FOUND.into_response();
        };
        let Some(file) = EmbeddedConsoleAssets::get(&key) else {
            return StatusCode::NOT_FOUND.into_response();
        };

        let mime = mime_guess::from_path(&key).first_or_octet_stream();
        let mut res = file.data.into_response();
        res.headers_mut().insert(
            header::CONTENT_TYPE,
            HeaderValue::from_str(mime.as_ref())
                .unwrap_or_else(|_| HeaderValue::from_static("application/octet-stream")),
        );
        *res.status_mut() = StatusCode::OK;
        res
    }
}

/// Route builder for console delivery. Merged into [`crate::http::routes::build_router`]
/// alongside `/health` and `/v1/analyze` (task 3.1 / RouterIntegration).
///
/// Generic over `S` so it can merge into the stateful API router without changing
/// handler contracts. Does not call LicenseGate / analyze — static bytes only
/// (requirement 6.2).
pub fn static_ui_router<S>() -> Router<S>
where
    S: Clone + Send + Sync + 'static,
{
    Router::new()
        .route("/", get(StaticUiHandler::redirect_root))
        .route("/ui", get(StaticUiHandler::index))
        .route("/ui/", get(StaticUiHandler::index))
        // axum 0.7 wildcard syntax (0.8+ uses `{*path}`).
        .route("/ui/*path", get(StaticUiHandler::asset))
}

/// Map a request path segment to an embed key (`index.html`, `console.css`, …).
/// Rejects empty, absolute, and `..` traversal paths.
fn normalize_asset_key(path: &str) -> Option<String> {
    let trimmed = path.trim_start_matches('/');
    if trimmed.is_empty() {
        return Some("index.html".to_string());
    }
    if trimmed.contains("..") || trimmed.starts_with('/') {
        return None;
    }
    // Trailing slash under /ui/ → index (e.g. /ui/subdir/).
    if trimmed.ends_with('/') {
        return Some(format!("{trimmed}index.html"));
    }
    Some(trimmed.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::license::{
        LicenseCheckResult, LicenseClient, LicenseError, LicenseGate, LicenseMeterResult,
        MockLicenseClient, MockOutcome, GLOBAL_TEST_LOCK,
    };
    use axum::body::{to_bytes, Body};
    use axum::http::{header, Request, StatusCode};
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use tower::ServiceExt;

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

    async fn get(app: &Router, uri: &str) -> axum::response::Response {
        app.clone()
            .oneshot(Request::builder().uri(uri).body(Body::empty()).unwrap())
            .await
            .expect("oneshot")
    }

    fn content_type(res: &axum::response::Response) -> String {
        res.headers()
            .get(header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("")
            .to_string()
    }

    #[tokio::test]
    async fn get_ui_slash_returns_200_html_index() {
        // Requirements 1.1, 1.2 — console path serves operable HTML from embed.
        let app = static_ui_router();
        let res = get(&app, "/ui/").await;
        assert_eq!(res.status(), StatusCode::OK);
        let ct = content_type(&res);
        assert!(
            ct.starts_with("text/html"),
            "Content-Type must be text/html, got {ct}"
        );
        let body = to_bytes(res.into_body(), 1024 * 1024).await.expect("body");
        let text = String::from_utf8(body.to_vec()).expect("utf8");
        assert!(
            text.to_ascii_lowercase().contains("<html"),
            "body must be HTML index"
        );
    }

    #[tokio::test]
    async fn get_ui_without_trailing_slash_serves_index() {
        let app = static_ui_router();
        let res = get(&app, "/ui").await;
        assert_eq!(res.status(), StatusCode::OK);
        let ct = content_type(&res);
        assert!(ct.starts_with("text/html"), "got {ct}");
    }

    #[tokio::test]
    async fn get_ui_assets_return_correct_content_types() {
        // Design: Content-Type from path; known assets console.css / console.js.
        let app = static_ui_router();

        let css = get(&app, "/ui/console.css").await;
        assert_eq!(css.status(), StatusCode::OK);
        let css_ct = content_type(&css);
        assert!(
            css_ct.starts_with("text/css"),
            "console.css Content-Type, got {css_ct}"
        );
        let css_body = to_bytes(css.into_body(), 1024 * 1024).await.expect("css");
        assert!(!css_body.is_empty());

        let js = get(&app, "/ui/console.js").await;
        assert_eq!(js.status(), StatusCode::OK);
        let js_ct = content_type(&js);
        assert!(
            js_ct.starts_with("text/javascript")
                || js_ct.starts_with("application/javascript")
                || js_ct.starts_with("application/ecmascript"),
            "console.js Content-Type, got {js_ct}"
        );
        let js_body = to_bytes(js.into_body(), 1024 * 1024).await.expect("js");
        assert!(!js_body.is_empty());
    }

    #[tokio::test]
    async fn missing_asset_returns_404() {
        let app = static_ui_router();
        let res = get(&app, "/ui/does-not-exist.xyz").await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn root_redirects_to_ui() {
        // Design optional: GET / → /ui/
        let app = static_ui_router();
        let res = get(&app, "/").await;
        assert!(
            res.status().is_redirection(),
            "expected 3xx redirect, got {}",
            res.status()
        );
        let loc = res
            .headers()
            .get(header::LOCATION)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        assert_eq!(loc, "/ui/");
    }

    #[tokio::test]
    async fn ui_path_is_distinct_from_health_and_analyze() {
        // Requirement 1.3 — console delivery path ≠ health / analyze.
        assert_ne!("/ui/", "/health");
        assert_ne!("/ui/", "/v1/analyze");

        let app = static_ui_router();
        let health = get(&app, "/health").await;
        assert_eq!(
            health.status(),
            StatusCode::NOT_FOUND,
            "static UI router must not serve /health"
        );
        let analyze = get(&app, "/v1/analyze").await;
        assert_eq!(
            analyze.status(),
            StatusCode::NOT_FOUND,
            "static UI router must not serve /v1/analyze"
        );
        let ui = get(&app, "/ui/").await;
        assert_eq!(ui.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn static_delivery_does_not_call_license_meter() {
        // Requirement 6.2 — static delivery must not meter.
        let check_calls = Arc::new(AtomicUsize::new(0));
        let meter_calls = Arc::new(AtomicUsize::new(0));

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

        let app = static_ui_router();
        for uri in ["/ui/", "/ui/console.css", "/ui/console.js", "/"] {
            let _ = get(&app, uri).await;
        }

        {
            let _guard = GLOBAL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
            assert_eq!(
                meter_calls.load(Ordering::SeqCst),
                0,
                "static UI must not call authorize_and_meter"
            );
            assert_eq!(
                check_calls.load(Ordering::SeqCst),
                0,
                "static UI must not call check_validity"
            );
            LicenseGate::clear_for_test();
        }
    }

    #[tokio::test]
    async fn path_traversal_is_rejected() {
        let app = static_ui_router();
        let res = get(&app, "/ui/../Cargo.toml").await;
        assert_eq!(res.status(), StatusCode::NOT_FOUND);
    }

    #[test]
    fn normalize_rejects_dotdot_and_accepts_index() {
        assert_eq!(normalize_asset_key(""), Some("index.html".into()));
        assert_eq!(
            normalize_asset_key("console.css"),
            Some("console.css".into())
        );
        assert_eq!(normalize_asset_key(".."), None);
        assert_eq!(normalize_asset_key("foo/../bar"), None);
    }
}
