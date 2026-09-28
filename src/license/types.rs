//! License error distinctions, failure reasons, and server response value types.
//!
//! Distinguishes startup failure from inference denial (and config), and carries
//! a machine-readable [`LicenseFailureReason`] for every startup/inference failure.
//! Transport / timeout failures are not a public variant: map them into
//! [`LicenseError::StartupFailed`] (check context) or
//! [`LicenseError::InferenceDenied`] (meter context) with
//! [`LicenseFailureReason::TemporaryFailure`].
//! [`LicenseError`] summaries must never include secrets such as license keys;
//! Debug/Display expose only the variant, reason, and caller-supplied summary text.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Why a license startup check or inference authorization failed.
///
/// The first six mirror the license server's client-facing error codes; the last
/// two are client-side. [`Self::code`] strings are a cross-spec contract.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LicenseFailureReason {
    /// `invalid_request`: key missing or malformed (key configuration problem).
    InvalidRequest,
    /// `license_invalid`: unregistered key.
    LicenseInvalid,
    /// `license_suspended`: license is suspended.
    LicenseSuspended,
    /// `monthly_limit_reached`: monthly usage limit reached (metering only).
    MonthlyLimitReached,
    /// `rate_limited`: too many requests (retryable).
    RateLimited,
    /// `temporary_failure`: server-side temporary failure, unreachable, or timeout (retryable).
    TemporaryFailure,
    /// `unexpected_response`: unparseable response, unknown code, or success-shape mismatch.
    UnexpectedResponse,
    /// `gate_not_installed`: metering path reached without an installed `LicenseGate`.
    GateNotInstalled,
}

impl LicenseFailureReason {
    /// Stable snake_case reason code.
    pub fn code(self) -> &'static str {
        match self {
            Self::InvalidRequest => "invalid_request",
            Self::LicenseInvalid => "license_invalid",
            Self::LicenseSuspended => "license_suspended",
            Self::MonthlyLimitReached => "monthly_limit_reached",
            Self::RateLimited => "rate_limited",
            Self::TemporaryFailure => "temporary_failure",
            Self::UnexpectedResponse => "unexpected_response",
            Self::GateNotInstalled => "gate_not_installed",
        }
    }

    /// Whether the caller may retry later (rate limit or temporary failure).
    pub fn is_retryable(self) -> bool {
        matches!(self, Self::RateLimited | Self::TemporaryFailure)
    }

    /// Classify a failed server response by its `error.code` and HTTP status.
    ///
    /// Known client codes map directly. Admin-only codes (`unauthorized`,
    /// `license_not_found`) are unexpected on client endpoints. Unknown or missing
    /// codes fall back to the status: 429 → rate limited, 5xx → temporary failure,
    /// anything else → unexpected response.
    pub fn from_server(code: Option<&str>, http_status: u16) -> Self {
        match code {
            Some("invalid_request") => Self::InvalidRequest,
            Some("license_invalid") => Self::LicenseInvalid,
            Some("license_suspended") => Self::LicenseSuspended,
            Some("monthly_limit_reached") => Self::MonthlyLimitReached,
            Some("rate_limited") => Self::RateLimited,
            Some("temporary_failure") => Self::TemporaryFailure,
            Some("unauthorized") | Some("license_not_found") => Self::UnexpectedResponse,
            _ => match http_status {
                429 => Self::RateLimited,
                500..=599 => Self::TemporaryFailure,
                _ => Self::UnexpectedResponse,
            },
        }
    }
}

/// Reason plus a secret-free summary (server message or transport summary).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LicenseFailure {
    pub reason: LicenseFailureReason,
    pub message: String,
}

impl LicenseFailure {
    pub fn new(reason: LicenseFailureReason, message: impl Into<String>) -> Self {
        Self {
            reason,
            message: message.into(),
        }
    }
}

/// Errors from license configuration, startup checks, or inference metering.
///
/// Display is one line with the failure category, reason code, and summary so
/// operators can distinguish startup failure from inference denial and see why
/// (requirements 5.1–5.3, 11.x). Callers must not put license keys or other
/// secrets into the summary strings (7.1).
///
/// Transport / timeout / communication failures are folded into the call-site
/// category: check → [`Self::StartupFailed`], meter → [`Self::InferenceDenied`].
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LicenseError {
    /// Process startup validity check failed (deny, transport, timeout, or related).
    #[error("license startup failed ({}): {}", .0.reason.code(), .0.message)]
    StartupFailed(LicenseFailure),

    /// Per-job authorize/meter failed; inference must not proceed
    /// (deny, transport, timeout, or related).
    #[error("license inference denied ({}): {}", .0.reason.code(), .0.message)]
    InferenceDenied(LicenseFailure),

    /// Invalid or missing license configuration (fail-closed).
    #[error("license config error: {0}")]
    Config(String),
}

impl LicenseError {
    /// Failure reason for startup/inference failures; `None` for config errors.
    pub fn reason(&self) -> Option<LicenseFailureReason> {
        match self {
            Self::StartupFailed(failure) | Self::InferenceDenied(failure) => Some(failure.reason),
            Self::Config(_) => None,
        }
    }

    /// Whether the failure may succeed if retried later.
    pub fn is_retryable(&self) -> bool {
        matches!(self.reason(), Some(reason) if reason.is_retryable())
    }
}

/// Result of a license validity check.
///
/// `monthly_limit == Some(0)` means unlimited, never "limit reached".
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LicenseCheckResult {
    pub allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub monthly_limit: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Usage counters reported by a successful metering call.
///
/// `monthly_limit == 0` means unlimited; `remaining` is `None` for unlimited
/// licenses and must not be treated as a denial.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct UsageSnapshot {
    pub used: u64,
    pub monthly_limit: u64,
    #[serde(default)]
    pub remaining: Option<u64>,
}

impl UsageSnapshot {
    pub fn is_unlimited(&self) -> bool {
        self.monthly_limit == 0
    }
}

/// Result of authorize-and-meter.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct LicenseMeterResult {
    pub allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub usage: Option<UsageSnapshot>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{
        LicenseCheckResult, LicenseError, LicenseFailure, LicenseFailureReason, LicenseMeterResult,
        UsageSnapshot,
    };

    const ALL_REASONS: [LicenseFailureReason; 8] = [
        LicenseFailureReason::InvalidRequest,
        LicenseFailureReason::LicenseInvalid,
        LicenseFailureReason::LicenseSuspended,
        LicenseFailureReason::MonthlyLimitReached,
        LicenseFailureReason::RateLimited,
        LicenseFailureReason::TemporaryFailure,
        LicenseFailureReason::UnexpectedResponse,
        LicenseFailureReason::GateNotInstalled,
    ];

    fn startup(reason: LicenseFailureReason, message: &str) -> LicenseError {
        LicenseError::StartupFailed(LicenseFailure::new(reason, message))
    }

    fn inference(reason: LicenseFailureReason, message: &str) -> LicenseError {
        LicenseError::InferenceDenied(LicenseFailure::new(reason, message))
    }

    #[test]
    fn reason_codes_are_stable_snake_case_strings() {
        let expected = [
            (LicenseFailureReason::InvalidRequest, "invalid_request"),
            (LicenseFailureReason::LicenseInvalid, "license_invalid"),
            (LicenseFailureReason::LicenseSuspended, "license_suspended"),
            (
                LicenseFailureReason::MonthlyLimitReached,
                "monthly_limit_reached",
            ),
            (LicenseFailureReason::RateLimited, "rate_limited"),
            (LicenseFailureReason::TemporaryFailure, "temporary_failure"),
            (
                LicenseFailureReason::UnexpectedResponse,
                "unexpected_response",
            ),
            (LicenseFailureReason::GateNotInstalled, "gate_not_installed"),
        ];
        assert_eq!(expected.len(), ALL_REASONS.len());
        for (reason, code) in expected {
            assert_eq!(reason.code(), code, "{reason:?}");
        }
    }

    #[test]
    fn only_rate_limited_and_temporary_failure_are_retryable() {
        for reason in ALL_REASONS {
            let expected = matches!(
                reason,
                LicenseFailureReason::RateLimited | LicenseFailureReason::TemporaryFailure
            );
            assert_eq!(reason.is_retryable(), expected, "{reason:?}");
        }
    }

    #[test]
    fn from_server_maps_the_six_client_codes_directly() {
        let cases = [
            ("invalid_request", 400, LicenseFailureReason::InvalidRequest),
            ("license_invalid", 401, LicenseFailureReason::LicenseInvalid),
            (
                "license_suspended",
                403,
                LicenseFailureReason::LicenseSuspended,
            ),
            (
                "monthly_limit_reached",
                403,
                LicenseFailureReason::MonthlyLimitReached,
            ),
            ("rate_limited", 429, LicenseFailureReason::RateLimited),
            (
                "temporary_failure",
                503,
                LicenseFailureReason::TemporaryFailure,
            ),
        ];
        for (code, status, expected) in cases {
            assert_eq!(
                LicenseFailureReason::from_server(Some(code), status),
                expected,
                "{code} / {status}"
            );
            // The server code wins over the HTTP status when it is known.
            assert_eq!(
                LicenseFailureReason::from_server(Some(code), 500),
                expected,
                "{code} / 500"
            );
        }
    }

    #[test]
    fn from_server_treats_admin_codes_as_unexpected_on_client_paths() {
        for code in ["unauthorized", "license_not_found"] {
            for status in [401, 404, 429, 503] {
                assert_eq!(
                    LicenseFailureReason::from_server(Some(code), status),
                    LicenseFailureReason::UnexpectedResponse,
                    "{code} / {status}"
                );
            }
        }
    }

    #[test]
    fn from_server_unknown_code_falls_back_to_http_status() {
        let cases = [
            (429, LicenseFailureReason::RateLimited),
            (503, LicenseFailureReason::TemporaryFailure),
            (502, LicenseFailureReason::TemporaryFailure),
            (500, LicenseFailureReason::TemporaryFailure),
            (400, LicenseFailureReason::UnexpectedResponse),
            (404, LicenseFailureReason::UnexpectedResponse),
        ];
        for (status, expected) in cases {
            assert_eq!(
                LicenseFailureReason::from_server(Some("brand_new_code"), status),
                expected,
                "unknown code / {status}"
            );
        }
    }

    #[test]
    fn from_server_without_code_falls_back_to_http_status() {
        let cases = [
            (429, LicenseFailureReason::RateLimited),
            (503, LicenseFailureReason::TemporaryFailure),
            (502, LicenseFailureReason::TemporaryFailure),
            (400, LicenseFailureReason::UnexpectedResponse),
            (200, LicenseFailureReason::UnexpectedResponse),
        ];
        for (status, expected) in cases {
            assert_eq!(
                LicenseFailureReason::from_server(None, status),
                expected,
                "no code / {status}"
            );
        }
    }

    #[test]
    fn startup_failed_is_distinguishable_from_inference_denied() {
        let startup = startup(LicenseFailureReason::LicenseInvalid, "rejected");
        let inference = inference(LicenseFailureReason::LicenseInvalid, "rejected");
        let config = LicenseError::Config("missing server_url".into());

        assert!(matches!(startup, LicenseError::StartupFailed(_)));
        assert!(matches!(inference, LicenseError::InferenceDenied(_)));
        assert!(matches!(config, LicenseError::Config(_)));
        assert_ne!(startup, inference);
        assert_ne!(startup, config);
        assert_ne!(inference, config);
    }

    #[test]
    fn display_shows_category_reason_code_and_message_on_one_line() {
        let startup = startup(
            LicenseFailureReason::LicenseSuspended,
            "The license is suspended.",
        );
        let inference = inference(
            LicenseFailureReason::MonthlyLimitReached,
            "The monthly usage limit has been reached.",
        );
        let config = LicenseError::Config("invalid timeout".into());

        assert_eq!(
            startup.to_string(),
            "license startup failed (license_suspended): The license is suspended."
        );
        assert_eq!(
            inference.to_string(),
            "license inference denied (monthly_limit_reached): The monthly usage limit has been reached."
        );
        assert_eq!(config.to_string(), "license config error: invalid timeout");
        for msg in [startup.to_string(), inference.to_string()] {
            assert!(!msg.contains('\n'), "Display must be one line: {msg}");
        }
    }

    #[test]
    fn transport_failures_surface_as_temporary_failure_in_call_site_category() {
        let check = startup(LicenseFailureReason::TemporaryFailure, "request timed out");
        let meter = inference(LicenseFailureReason::TemporaryFailure, "connection failed");

        let check_msg = check.to_string();
        let meter_msg = meter.to_string();
        assert!(
            check_msg.contains("startup failed")
                && check_msg.contains("temporary_failure")
                && check_msg.contains("request timed out"),
            "{check_msg}"
        );
        assert!(
            meter_msg.contains("inference denied")
                && meter_msg.contains("temporary_failure")
                && meter_msg.contains("connection failed"),
            "{meter_msg}"
        );
    }

    #[test]
    fn license_error_exposes_reason_and_retryability() {
        let limited = inference(LicenseFailureReason::RateLimited, "Too many requests.");
        assert_eq!(limited.reason(), Some(LicenseFailureReason::RateLimited));
        assert!(limited.is_retryable());

        let temporary = startup(LicenseFailureReason::TemporaryFailure, "request timed out");
        assert_eq!(
            temporary.reason(),
            Some(LicenseFailureReason::TemporaryFailure)
        );
        assert!(temporary.is_retryable());

        let suspended = startup(LicenseFailureReason::LicenseSuspended, "suspended");
        assert_eq!(
            suspended.reason(),
            Some(LicenseFailureReason::LicenseSuspended)
        );
        assert!(!suspended.is_retryable());

        let not_installed = inference(
            LicenseFailureReason::GateNotInstalled,
            "license gate not installed",
        );
        assert_eq!(
            not_installed.reason(),
            Some(LicenseFailureReason::GateNotInstalled)
        );
        assert!(!not_installed.is_retryable());

        let config = LicenseError::Config("missing server_url".into());
        assert_eq!(config.reason(), None);
        assert!(!config.is_retryable());
    }

    #[test]
    fn debug_and_display_do_not_hold_or_emit_key_fields() {
        // LicenseError stores only reason + caller-supplied summary; no key field exists.
        let errors = [
            LicenseError::Config("missing required key".into()),
            startup(LicenseFailureReason::InvalidRequest, "Invalid request."),
            inference(LicenseFailureReason::LicenseInvalid, "Invalid license."),
        ];
        for err in errors {
            let debug = format!("{err:?}").to_lowercase();
            let display = err.to_string().to_lowercase();
            for field in ["api_key", "license_key"] {
                assert!(!debug.contains(field), "Debug exposes {field}: {debug}");
                assert!(
                    !display.contains(field),
                    "Display exposes {field}: {display}"
                );
            }
        }

        let check = LicenseCheckResult {
            allowed: false,
            message: Some("denied".into()),
            ..Default::default()
        };
        let meter = LicenseMeterResult {
            allowed: true,
            ..Default::default()
        };
        let check_dbg = format!("{check:?}").to_lowercase();
        let meter_dbg = format!("{meter:?}").to_lowercase();
        assert!(!check_dbg.contains("api_key") && !check_dbg.contains("license_key"));
        assert!(!meter_dbg.contains("api_key") && !meter_dbg.contains("license_key"));
    }

    #[test]
    fn check_result_carries_status_and_monthly_limit() {
        let check = LicenseCheckResult {
            allowed: true,
            status: Some("active".into()),
            monthly_limit: Some(0),
            message: Some("ok".into()),
        };
        assert!(check.allowed);
        assert_eq!(check.status.as_deref(), Some("active"));
        assert_eq!(check.monthly_limit, Some(0));
        assert_eq!(check.message.as_deref(), Some("ok"));

        let default = LicenseCheckResult::default();
        assert!(!default.allowed, "default must not allow (fail-closed)");
        assert!(default.status.is_none());
        assert!(default.monthly_limit.is_none());
        assert!(default.message.is_none());
    }

    #[test]
    fn meter_result_carries_optional_usage_snapshot() {
        let meter = LicenseMeterResult {
            allowed: true,
            usage: Some(UsageSnapshot {
                used: 3,
                monthly_limit: 100,
                remaining: Some(97),
            }),
            message: None,
        };
        let usage = meter.usage.as_ref().expect("usage");
        assert_eq!(usage.used, 3);
        assert_eq!(usage.monthly_limit, 100);
        assert_eq!(usage.remaining, Some(97));
        assert!(!usage.is_unlimited());

        let default = LicenseMeterResult::default();
        assert!(!default.allowed, "default must not allow (fail-closed)");
        assert!(default.usage.is_none());
        assert!(default.message.is_none());
    }

    #[test]
    fn usage_snapshot_with_zero_monthly_limit_is_unlimited_and_needs_no_remaining() {
        let unlimited = UsageSnapshot {
            used: 42,
            monthly_limit: 0,
            remaining: None,
        };
        assert!(unlimited.is_unlimited());
        assert!(unlimited.remaining.is_none());

        let limited = UsageSnapshot {
            used: 1,
            monthly_limit: 1,
            remaining: Some(0),
        };
        assert!(!limited.is_unlimited());
    }
}
