//! HTTP failure categories mapped to status codes and JSON error bodies.
//!
//! Pure mapping types for later handlers/middleware (routes are not wired here).

use crate::analyze::AnalyzeError;
use crate::license::{LicenseError, LicenseFailureReason};
use crate::preprocess::PreprocessError;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde::Serialize;
use thiserror::Error;

/// Call-site distinguishable HTTP failures (requirements 8.1–8.4).
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum HttpError {
    /// Client-caused invalid input (400 / `invalid_input`).
    #[error("{0}")]
    InvalidInput(String),
    /// Request body exceeds configured limit (413 / `payload_too_large`).
    #[error("{0}")]
    PayloadTooLarge(String),
    /// License authorize/meter denied inference (403 / `license_inference_denied`).
    #[error("{0}")]
    LicenseInferenceDenied(String),
    /// License server rate-limited the request; retry later (429 / `license_rate_limited`).
    #[error("{0}")]
    LicenseRateLimited(String),
    /// License server temporarily unavailable; retry later
    /// (503 / `license_temporarily_unavailable`).
    #[error("{0}")]
    LicenseTemporarilyUnavailable(String),
    /// End-to-end request processing timed out (504 / `request_timeout`).
    #[error("{0}")]
    RequestTimeout(String),
    /// Unexpected server-side failure (500 / `internal_error`).
    #[error("{0}")]
    Internal(String),
}

/// JSON error envelope: `{ "error": { "code", "message" } }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorBody {
    pub error: ErrorDetail,
}

/// Failure code + caller-safe summary (no secret plaintext).
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct ErrorDetail {
    pub code: &'static str,
    pub message: String,
}

impl HttpError {
    pub fn invalid_input(msg: impl Into<String>) -> Self {
        Self::InvalidInput(sanitize_public_message(&msg.into()))
    }

    pub fn payload_too_large(msg: impl Into<String>) -> Self {
        Self::PayloadTooLarge(sanitize_public_message(&msg.into()))
    }

    pub fn license_inference_denied(msg: impl Into<String>) -> Self {
        Self::LicenseInferenceDenied(sanitize_public_message(&msg.into()))
    }

    pub fn license_rate_limited(msg: impl Into<String>) -> Self {
        Self::LicenseRateLimited(sanitize_public_message(&msg.into()))
    }

    pub fn license_temporarily_unavailable(msg: impl Into<String>) -> Self {
        Self::LicenseTemporarilyUnavailable(sanitize_public_message(&msg.into()))
    }

    pub fn request_timeout(msg: impl Into<String>) -> Self {
        Self::RequestTimeout(sanitize_public_message(&msg.into()))
    }

    pub fn internal(msg: impl Into<String>) -> Self {
        Self::Internal(sanitize_public_message(&msg.into()))
    }

    /// HTTP status for this failure category.
    pub fn status_code(&self) -> StatusCode {
        match self {
            Self::InvalidInput(_) => StatusCode::BAD_REQUEST,
            Self::PayloadTooLarge(_) => StatusCode::PAYLOAD_TOO_LARGE,
            Self::LicenseInferenceDenied(_) => StatusCode::FORBIDDEN,
            Self::LicenseRateLimited(_) => StatusCode::TOO_MANY_REQUESTS,
            Self::LicenseTemporarilyUnavailable(_) => StatusCode::SERVICE_UNAVAILABLE,
            Self::RequestTimeout(_) => StatusCode::GATEWAY_TIMEOUT,
            Self::Internal(_) => StatusCode::INTERNAL_SERVER_ERROR,
        }
    }

    /// Stable machine-readable `error.code` (design mapping table).
    pub fn error_code(&self) -> &'static str {
        match self {
            Self::InvalidInput(_) => "invalid_input",
            Self::PayloadTooLarge(_) => "payload_too_large",
            Self::LicenseInferenceDenied(_) => "license_inference_denied",
            Self::LicenseRateLimited(_) => "license_rate_limited",
            Self::LicenseTemporarilyUnavailable(_) => "license_temporarily_unavailable",
            Self::RequestTimeout(_) => "request_timeout",
            Self::Internal(_) => "internal_error",
        }
    }

    /// Public summary safe for response bodies.
    pub fn message(&self) -> &str {
        match self {
            Self::InvalidInput(m)
            | Self::PayloadTooLarge(m)
            | Self::LicenseInferenceDenied(m)
            | Self::LicenseRateLimited(m)
            | Self::LicenseTemporarilyUnavailable(m)
            | Self::RequestTimeout(m)
            | Self::Internal(m) => m,
        }
    }

    pub fn to_error_body(&self) -> ErrorBody {
        ErrorBody {
            error: ErrorDetail {
                code: self.error_code(),
                message: self.message().to_string(),
            },
        }
    }
}

impl From<LicenseError> for HttpError {
    fn from(err: LicenseError) -> Self {
        match err {
            LicenseError::InferenceDenied(failure) => {
                let msg = format!("{}: {}", failure.reason.code(), failure.message);
                match failure.reason {
                    LicenseFailureReason::RateLimited => Self::license_rate_limited(msg),
                    LicenseFailureReason::TemporaryFailure => {
                        Self::license_temporarily_unavailable(msg)
                    }
                    LicenseFailureReason::InvalidRequest
                    | LicenseFailureReason::LicenseInvalid
                    | LicenseFailureReason::LicenseSuspended
                    | LicenseFailureReason::MonthlyLimitReached
                    | LicenseFailureReason::UnexpectedResponse
                    | LicenseFailureReason::GateNotInstalled => Self::license_inference_denied(msg),
                }
            }
            LicenseError::StartupFailed(failure) => {
                Self::internal(format!("license startup failed: {}", failure.message))
            }
            LicenseError::Config(msg) => Self::internal(format!("license config error: {msg}")),
        }
    }
}

impl From<AnalyzeError> for HttpError {
    fn from(err: AnalyzeError) -> Self {
        match err {
            AnalyzeError::Preprocess(pe) => match pe {
                PreprocessError::Invalid(msg) => Self::invalid_input(msg),
                PreprocessError::Io(io) => Self::invalid_input(io.to_string()),
            },
            AnalyzeError::License(le) => le.into(),
            AnalyzeError::Infer(ie) => Self::internal(ie.to_string()),
            AnalyzeError::Post(msg) => Self::internal(msg),
            AnalyzeError::Io(io) => Self::internal(io.to_string()),
            AnalyzeError::Csv(ce) => Self::internal(ce.to_string()),
        }
    }
}

impl IntoResponse for HttpError {
    fn into_response(self) -> Response {
        (self.status_code(), Json(self.to_error_body())).into_response()
    }
}

/// Keys whose `key=value` assignments are redacted from public error text.
const SECRET_ASSIGNMENT_KEYS: [&str; 2] = ["api_key", "license_key"];

/// Prefix of license key tokens (`lk_` + hex).
const LICENSE_KEY_TOKEN_PREFIX: &str = "lk_";

/// Strip secret-looking plaintext (`api_key=...`, `license_key=...`, `lk_...`
/// tokens, bearer tokens) from public error text.
fn sanitize_public_message(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let lower = raw.to_ascii_lowercase();
    let bytes = raw.as_bytes();
    let mut i = 0;
    'scan: while i < bytes.len() {
        if let Some(rest) = lower.get(i..) {
            for key in SECRET_ASSIGNMENT_KEYS {
                if rest.starts_with(key) {
                    let mut j = i + key.len();
                    while j < bytes.len() && (bytes[j] == b' ' || bytes[j] == b'\t') {
                        j += 1;
                    }
                    if j < bytes.len() && bytes[j] == b'=' {
                        j += 1;
                        while j < bytes.len()
                            && !bytes[j].is_ascii_whitespace()
                            && bytes[j] != b','
                            && bytes[j] != b';'
                            && bytes[j] != b'}'
                        {
                            j += 1;
                        }
                        out.push_str(key);
                        out.push_str("=[REDACTED]");
                        i = j;
                        continue 'scan;
                    }
                }
            }
            let at_word_start =
                i == 0 || !(bytes[i - 1].is_ascii_alphanumeric() || bytes[i - 1] == b'_');
            if at_word_start && rest.starts_with(LICENSE_KEY_TOKEN_PREFIX) {
                let mut j = i + LICENSE_KEY_TOKEN_PREFIX.len();
                while j < bytes.len()
                    && (bytes[j].is_ascii_alphanumeric() || bytes[j] == b'_' || bytes[j] == b'-')
                {
                    j += 1;
                }
                out.push_str("lk_[REDACTED]");
                i = j;
                continue;
            }
            if rest.starts_with("bearer ") {
                out.push_str("Bearer [REDACTED]");
                i += "bearer ".len();
                while i < bytes.len() && !bytes[i].is_ascii_whitespace() {
                    i += 1;
                }
                continue;
            }
        }
        let ch = raw[i..].chars().next().unwrap();
        out.push(ch);
        i += ch.len_utf8();
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::license::{LicenseFailure, LicenseFailureReason};
    use crate::phase2::InferError;
    use axum::body::to_bytes;
    use axum::response::IntoResponse;
    use serde_json::Value;

    #[test]
    fn maps_invalid_input_to_400() {
        let err = HttpError::invalid_input("missing ecl field");
        assert_eq!(err.status_code(), StatusCode::BAD_REQUEST);
        assert_eq!(err.error_code(), "invalid_input");
        let body = err.to_error_body();
        assert_eq!(body.error.code, "invalid_input");
        assert!(body.error.message.contains("missing ecl"));
    }

    #[test]
    fn maps_payload_too_large_to_413() {
        let err = HttpError::payload_too_large("body exceeds max_body_bytes");
        assert_eq!(err.status_code(), StatusCode::PAYLOAD_TOO_LARGE);
        assert_eq!(err.error_code(), "payload_too_large");
    }

    #[test]
    fn maps_license_inference_denied_to_403() {
        let err = HttpError::license_inference_denied("quota exceeded");
        assert_eq!(err.status_code(), StatusCode::FORBIDDEN);
        assert_eq!(err.error_code(), "license_inference_denied");
    }

    #[test]
    fn maps_license_rate_limited_to_429() {
        let err = HttpError::license_rate_limited("rate_limited: slow down");
        assert_eq!(err.status_code(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(err.error_code(), "license_rate_limited");
    }

    #[test]
    fn maps_license_temporarily_unavailable_to_503() {
        let err = HttpError::license_temporarily_unavailable("temporary_failure: timed out");
        assert_eq!(err.status_code(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(err.error_code(), "license_temporarily_unavailable");
    }

    #[test]
    fn maps_request_timeout_to_504() {
        let err = HttpError::request_timeout("request_timeout_secs exceeded");
        assert_eq!(err.status_code(), StatusCode::GATEWAY_TIMEOUT);
        assert_eq!(err.error_code(), "request_timeout");
    }

    #[test]
    fn maps_internal_to_500() {
        let err = HttpError::internal("unexpected failure");
        assert_eq!(err.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(err.error_code(), "internal_error");
    }

    #[test]
    fn license_inference_denied_from_license_error() {
        let err: HttpError = LicenseError::InferenceDenied(LicenseFailure::new(
            LicenseFailureReason::MonthlyLimitReached,
            "meter rejected: quota exceeded",
        ))
        .into();
        assert_eq!(err.status_code(), StatusCode::FORBIDDEN);
        assert_eq!(err.error_code(), "license_inference_denied");
        assert!(err.message().contains("quota exceeded"));
    }

    #[test]
    fn analyze_preprocess_invalid_maps_to_invalid_input() {
        let err: HttpError =
            AnalyzeError::Preprocess(PreprocessError::Invalid("bad ecl filename".into())).into();
        assert_eq!(err.status_code(), StatusCode::BAD_REQUEST);
        assert_eq!(err.error_code(), "invalid_input");
    }

    #[test]
    fn analyze_license_denied_maps_to_forbidden() {
        let err: HttpError =
            AnalyzeError::License(LicenseError::InferenceDenied(LicenseFailure::new(
                LicenseFailureReason::GateNotInstalled,
                "license gate not installed",
            )))
            .into();
        assert_eq!(err.status_code(), StatusCode::FORBIDDEN);
        assert_eq!(err.error_code(), "license_inference_denied");
    }

    #[test]
    fn analyze_infer_maps_to_internal() {
        let err: HttpError = AnalyzeError::Infer(InferError::EmptyModelBytes).into();
        assert_eq!(err.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(err.error_code(), "internal_error");
    }

    #[test]
    fn error_body_json_has_code_and_message_only() {
        let err = HttpError::invalid_input("empty upload");
        let json = serde_json::to_value(err.to_error_body()).expect("serialize");
        assert_eq!(json["error"]["code"], "invalid_input");
        assert_eq!(json["error"]["message"], "empty upload");
        let error_obj = json["error"].as_object().expect("error object");
        assert_eq!(error_obj.len(), 2);
    }

    #[tokio::test]
    async fn into_response_uses_status_and_error_envelope() {
        let err = HttpError::payload_too_large("too large");
        let response = err.into_response();
        assert_eq!(response.status(), StatusCode::PAYLOAD_TOO_LARGE);
        let bytes = to_bytes(response.into_body(), 1024).await.expect("body");
        let v: Value = serde_json::from_slice(&bytes).expect("json");
        assert_eq!(v["error"]["code"], "payload_too_large");
        assert!(v["error"]["message"]
            .as_str()
            .unwrap()
            .contains("too large"));
    }

    #[test]
    fn error_message_never_includes_api_key_plaintext() {
        let secret = "super-secret-token-xyz";
        let err = HttpError::internal(format!("upstream failed api_key={secret} detail"));
        let body = serde_json::to_string(&err.to_error_body()).expect("json");
        assert!(
            !body.contains(secret),
            "error body must not contain api_key plaintext: {body}"
        );
        assert!(
            !err.message().contains(secret),
            "message must redact secret: {}",
            err.message()
        );
        assert!(
            err.message().contains("api_key=[REDACTED]")
                || !err.message().to_ascii_lowercase().contains("api_key="),
            "expected redaction marker: {}",
            err.message()
        );
    }

    fn denied(reason: LicenseFailureReason, message: &str) -> HttpError {
        LicenseError::InferenceDenied(LicenseFailure::new(reason, message)).into()
    }

    #[test]
    fn inference_denied_reasons_map_by_retryability() {
        use LicenseFailureReason::*;
        let cases = [
            (
                InvalidRequest,
                StatusCode::FORBIDDEN,
                "license_inference_denied",
            ),
            (
                LicenseInvalid,
                StatusCode::FORBIDDEN,
                "license_inference_denied",
            ),
            (
                LicenseSuspended,
                StatusCode::FORBIDDEN,
                "license_inference_denied",
            ),
            (
                MonthlyLimitReached,
                StatusCode::FORBIDDEN,
                "license_inference_denied",
            ),
            (
                UnexpectedResponse,
                StatusCode::FORBIDDEN,
                "license_inference_denied",
            ),
            (
                GateNotInstalled,
                StatusCode::FORBIDDEN,
                "license_inference_denied",
            ),
            (
                RateLimited,
                StatusCode::TOO_MANY_REQUESTS,
                "license_rate_limited",
            ),
            (
                TemporaryFailure,
                StatusCode::SERVICE_UNAVAILABLE,
                "license_temporarily_unavailable",
            ),
        ];
        for (reason, status, code) in cases {
            let err = denied(reason, "server said no");
            assert_eq!(err.status_code(), status, "reason={}", reason.code());
            assert_eq!(err.error_code(), code, "reason={}", reason.code());
            let expected = format!("{}: server said no", reason.code());
            assert_eq!(err.message(), expected);
            let body = err.to_error_body();
            assert_eq!(body.error.code, code);
            assert_eq!(body.error.message, expected);
        }
    }

    #[test]
    fn inference_denied_message_matches_contract_examples() {
        assert_eq!(
            denied(
                LicenseFailureReason::MonthlyLimitReached,
                "The monthly usage limit has been reached."
            )
            .message(),
            "monthly_limit_reached: The monthly usage limit has been reached."
        );
        assert_eq!(
            denied(
                LicenseFailureReason::RateLimited,
                "Too many requests. Please retry later."
            )
            .message(),
            "rate_limited: Too many requests. Please retry later."
        );
        assert_eq!(
            denied(LicenseFailureReason::TemporaryFailure, "request timed out").message(),
            "temporary_failure: request timed out"
        );
    }

    #[tokio::test]
    async fn retryable_license_denials_render_status_and_envelope() {
        for (reason, status, code) in [
            (
                LicenseFailureReason::RateLimited,
                StatusCode::TOO_MANY_REQUESTS,
                "license_rate_limited",
            ),
            (
                LicenseFailureReason::TemporaryFailure,
                StatusCode::SERVICE_UNAVAILABLE,
                "license_temporarily_unavailable",
            ),
        ] {
            let response = denied(reason, "retry later").into_response();
            assert_eq!(response.status(), status);
            let bytes = to_bytes(response.into_body(), 1024).await.expect("body");
            let v: Value = serde_json::from_slice(&bytes).expect("json");
            assert_eq!(v["error"]["code"], code);
            assert_eq!(
                v["error"]["message"],
                format!("{}: retry later", reason.code())
            );
        }
    }

    #[test]
    fn startup_and_config_license_errors_stay_internal() {
        for reason in [
            LicenseFailureReason::LicenseInvalid,
            LicenseFailureReason::RateLimited,
            LicenseFailureReason::TemporaryFailure,
        ] {
            let err: HttpError =
                LicenseError::StartupFailed(LicenseFailure::new(reason, "verify failed")).into();
            assert_eq!(err.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
            assert_eq!(err.error_code(), "internal_error");
        }
        let err: HttpError = LicenseError::Config("license_key is required".into()).into();
        assert_eq!(err.status_code(), StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(err.error_code(), "internal_error");
    }

    const TEST_LICENSE_KEY: &str = "lk_0123456789abcdef0123456789abcdef";

    #[test]
    fn license_key_assignment_is_redacted() {
        let err = HttpError::internal(format!("bad config license_key={TEST_LICENSE_KEY}; retry"));
        assert!(
            !err.message().contains("0123456789abcdef"),
            "must not leak license key: {}",
            err.message()
        );
        assert_eq!(err.message(), "bad config license_key=[REDACTED]; retry");
    }

    #[test]
    fn bare_lk_token_is_redacted() {
        let err = HttpError::invalid_input(format!("key {TEST_LICENSE_KEY}. was rejected"));
        assert!(
            !err.message().contains("0123456789abcdef"),
            "must not leak license key: {}",
            err.message()
        );
        assert_eq!(err.message(), "key lk_[REDACTED]. was rejected");
    }

    #[test]
    fn lk_inside_a_word_is_not_redacted() {
        let err = HttpError::invalid_input("talk_back and bulk_data stay");
        assert_eq!(err.message(), "talk_back and bulk_data stay");
    }

    #[test]
    fn license_denial_message_redacts_license_key() {
        let err = denied(
            LicenseFailureReason::LicenseInvalid,
            &format!("unknown key {TEST_LICENSE_KEY}"),
        );
        assert!(
            !err.message().contains("0123456789abcdef"),
            "must not leak license key: {}",
            err.message()
        );
        assert_eq!(err.message(), "license_invalid: unknown key lk_[REDACTED]");
    }

    #[test]
    fn api_key_assignment_is_still_redacted() {
        let err = HttpError::internal("upstream api_key=abc123,next");
        assert_eq!(err.message(), "upstream api_key=[REDACTED],next");
    }

    #[test]
    fn bearer_token_plaintext_is_redacted() {
        let token = "sk-live-abcdef012345";
        let err = HttpError::invalid_input(format!("auth header Bearer {token} rejected"));
        assert!(
            !err.message().contains(token),
            "must not leak bearer token: {}",
            err.message()
        );
    }
}
