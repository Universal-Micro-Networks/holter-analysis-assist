//! License error distinctions and minimal server response types.
//!
//! Distinguishes startup failure from inference denial (and config).
//! Transport / timeout failures are not a public variant: map them into
//! [`LicenseError::StartupFailed`] (check context) or
//! [`LicenseError::InferenceDenied`] (meter context).
//! [`LicenseError`] summaries must never include secrets such as API keys;
//! Debug/Display expose only the variant and caller-supplied summary text.

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// Errors from license configuration, startup checks, or inference metering.
///
/// Display includes a failure category label plus a short summary so operators can
/// distinguish startup failure from inference denial (requirements 5.1–5.3).
/// Callers must not put API keys or other secrets into the summary strings (7.1).
///
/// Transport / timeout / communication failures are folded into the call-site
/// category: check → [`Self::StartupFailed`], meter → [`Self::InferenceDenied`].
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum LicenseError {
    /// Process startup validity check failed (deny, transport, timeout, or related).
    #[error("license startup failed: {0}")]
    StartupFailed(String),

    /// Per-job authorize/meter failed; inference must not proceed
    /// (deny, transport, timeout, or related).
    #[error("license inference denied: {0}")]
    InferenceDenied(String),

    /// Invalid or missing license configuration (fail-closed).
    #[error("license config error: {0}")]
    Config(String),
}

/// Minimal result of a license validity check (`allowed` + optional `message`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LicenseCheckResult {
    pub allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

/// Minimal result of authorize-and-meter (`allowed` + optional `message`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LicenseMeterResult {
    pub allowed: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::{LicenseCheckResult, LicenseError, LicenseMeterResult};

    #[test]
    fn startup_failed_is_distinguishable_from_inference_denied() {
        let startup = LicenseError::StartupFailed("validity check rejected".into());
        let inference = LicenseError::InferenceDenied("meter rejected".into());
        assert!(matches!(startup, LicenseError::StartupFailed(_)));
        assert!(matches!(inference, LicenseError::InferenceDenied(_)));
        assert_ne!(startup, inference);
    }

    #[test]
    fn public_variants_are_only_startup_inference_and_config() {
        let startup = LicenseError::StartupFailed("check denied".into());
        let inference = LicenseError::InferenceDenied("meter denied".into());
        let config = LicenseError::Config("missing server_url".into());

        assert!(matches!(startup, LicenseError::StartupFailed(_)));
        assert!(matches!(inference, LicenseError::InferenceDenied(_)));
        assert!(matches!(config, LicenseError::Config(_)));
        assert_ne!(startup, inference);
        assert_ne!(startup, config);
        assert_ne!(inference, config);
    }

    #[test]
    fn transport_and_timeout_map_into_check_or_meter_category() {
        // Error Categories: check-context transport/timeout → StartupFailed;
        // meter-context transport/timeout → InferenceDenied. No public Transport.
        let check_transport = LicenseError::StartupFailed("connection timed out".into());
        let meter_transport = LicenseError::InferenceDenied("dns failure".into());

        assert!(matches!(check_transport, LicenseError::StartupFailed(_)));
        assert!(matches!(meter_transport, LicenseError::InferenceDenied(_)));
        assert_ne!(check_transport, meter_transport);

        let check_msg = check_transport.to_string();
        let meter_msg = meter_transport.to_string();
        assert!(
            check_msg.contains("startup failed") && check_msg.contains("connection timed out"),
            "check-context transport must surface as StartupFailed: {check_msg}"
        );
        assert!(
            meter_msg.contains("inference denied") && meter_msg.contains("dns failure"),
            "meter-context transport must surface as InferenceDenied: {meter_msg}"
        );
    }

    #[test]
    fn display_includes_failure_category_and_summary() {
        let startup = LicenseError::StartupFailed("server denied".into());
        let inference = LicenseError::InferenceDenied("quota exceeded".into());
        let config = LicenseError::Config("invalid timeout".into());

        let startup_msg = startup.to_string();
        let inference_msg = inference.to_string();
        let config_msg = config.to_string();

        assert!(
            startup_msg.contains("startup failed") && startup_msg.contains("server denied"),
            "startup Display must include category and summary: {startup_msg}"
        );
        assert!(
            inference_msg.contains("inference denied") && inference_msg.contains("quota exceeded"),
            "inference Display must include category and summary: {inference_msg}"
        );
        assert!(
            config_msg.contains("config") && config_msg.contains("invalid timeout"),
            "config Display must include category and summary: {config_msg}"
        );
    }

    #[test]
    fn debug_and_display_do_not_hold_or_emit_api_key_fields() {
        // LicenseError stores only caller-supplied summaries — no api_key field.
        // Constructing with a secret-looking token must still not invent an "api_key=" field
        // in Debug; callers must keep secrets out of summaries (policy fixed at the type).
        let err = LicenseError::Config("missing required key".into());
        let debug = format!("{err:?}");
        let display = err.to_string();

        assert!(
            !debug.to_lowercase().contains("api_key"),
            "Debug must not expose an api_key field: {debug}"
        );
        assert!(
            !display.to_lowercase().contains("api_key"),
            "Display must not expose api_key: {display}"
        );

        let check = LicenseCheckResult {
            allowed: false,
            message: Some("denied".into()),
        };
        let meter = LicenseMeterResult {
            allowed: true,
            message: None,
        };
        let check_dbg = format!("{check:?}");
        let meter_dbg = format!("{meter:?}");
        assert!(!check_dbg.to_lowercase().contains("api_key"));
        assert!(!meter_dbg.to_lowercase().contains("api_key"));
    }

    #[test]
    fn check_and_meter_results_expose_allowed_and_optional_message() {
        let check = LicenseCheckResult {
            allowed: true,
            message: Some("ok".into()),
        };
        assert!(check.allowed);
        assert_eq!(check.message.as_deref(), Some("ok"));

        let meter = LicenseMeterResult {
            allowed: false,
            message: None,
        };
        assert!(!meter.allowed);
        assert!(meter.message.is_none());
    }
}
