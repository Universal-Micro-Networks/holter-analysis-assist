//! Swappable license-server port and a network-free test mock.
//!
//! [`LicenseClient`] is the boundary Gate (and tests) depend on.
//! [`MockLicenseClient`] scripts success / reject-with-reason / transport failure
//! without reaching an external server. Failures map by operation:
//! check → [`LicenseError::StartupFailed`], meter → [`LicenseError::InferenceDenied`],
//! carrying the scripted reason (transport → `temporary_failure`).

use super::types::{
    LicenseCheckResult, LicenseError, LicenseFailure, LicenseFailureReason, LicenseMeterResult,
};

/// Port for license-server operations (validity check and authorize+meter).
pub trait LicenseClient: Send + Sync {
    fn check_validity(&self) -> Result<LicenseCheckResult, LicenseError>;
    fn authorize_and_meter(&self) -> Result<LicenseMeterResult, LicenseError>;
}

/// Scripted outcome for one mock operation (no network).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MockOutcome {
    /// Server allows the operation.
    Success { message: Option<String> },
    /// Server rejects the operation for `reason`. `message: None` falls back to a
    /// fixed non-empty summary.
    Reject {
        reason: LicenseFailureReason,
        message: Option<String>,
    },
    /// Transport / timeout / communication failure (reported as `temporary_failure`).
    TransportFail { message: String },
}

impl MockOutcome {
    /// Failure for a non-success outcome; `None` for [`Self::Success`].
    fn failure(&self, default_reject_message: &str) -> Option<LicenseFailure> {
        match self {
            Self::Success { .. } => None,
            Self::Reject { reason, message } => Some(LicenseFailure::new(
                *reason,
                message
                    .clone()
                    .unwrap_or_else(|| default_reject_message.into()),
            )),
            Self::TransportFail { message } => Some(LicenseFailure::new(
                LicenseFailureReason::TemporaryFailure,
                message.clone(),
            )),
        }
    }

    fn success_message(&self) -> Option<String> {
        match self {
            Self::Success { message } => message.clone(),
            _ => None,
        }
    }
}

/// Test double that reproduces success / reject / transport without network I/O.
#[derive(Debug, Clone)]
pub struct MockLicenseClient {
    check: MockOutcome,
    meter: MockOutcome,
}

impl MockLicenseClient {
    /// Build a mock with independent scripts for check and meter.
    pub fn new(check: MockOutcome, meter: MockOutcome) -> Self {
        Self { check, meter }
    }
}

impl LicenseClient for MockLicenseClient {
    fn check_validity(&self) -> Result<LicenseCheckResult, LicenseError> {
        if let Some(failure) = self.check.failure("validity check rejected") {
            return Err(LicenseError::StartupFailed(failure));
        }
        Ok(LicenseCheckResult {
            allowed: true,
            message: self.check.success_message(),
            ..Default::default()
        })
    }

    fn authorize_and_meter(&self) -> Result<LicenseMeterResult, LicenseError> {
        if let Some(failure) = self.meter.failure("authorize and meter rejected") {
            return Err(LicenseError::InferenceDenied(failure));
        }
        Ok(LicenseMeterResult {
            allowed: true,
            message: self.meter.success_message(),
            ..Default::default()
        })
    }
}

#[cfg(test)]
mod tests {
    use super::{LicenseClient, MockLicenseClient, MockOutcome};
    use crate::license::types::{LicenseError, LicenseFailureReason};

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

    fn reject(reason: LicenseFailureReason, message: Option<&str>) -> MockOutcome {
        MockOutcome::Reject {
            reason,
            message: message.map(Into::into),
        }
    }

    #[test]
    fn reject_check_is_startup_failed_with_the_given_reason() {
        for reason in ALL_REASONS {
            let client = MockLicenseClient::new(
                reject(reason, Some("server said no")),
                MockOutcome::Success { message: None },
            );
            match client.check_validity() {
                Err(LicenseError::StartupFailed(f)) => {
                    assert_eq!(f.reason, reason);
                    assert_eq!(f.message, "server said no");
                }
                other => panic!("{reason:?}: expected StartupFailed, got {other:?}"),
            }
        }
    }

    #[test]
    fn reject_meter_is_inference_denied_with_the_given_reason() {
        for reason in ALL_REASONS {
            let client = MockLicenseClient::new(
                MockOutcome::Success { message: None },
                reject(reason, Some("server said no")),
            );
            match client.authorize_and_meter() {
                Err(LicenseError::InferenceDenied(f)) => {
                    assert_eq!(f.reason, reason);
                    assert_eq!(f.message, "server said no");
                }
                other => panic!("{reason:?}: expected InferenceDenied, got {other:?}"),
            }
        }
    }

    #[test]
    fn reject_without_message_uses_a_non_empty_default() {
        for reason in ALL_REASONS {
            let client = MockLicenseClient::new(reject(reason, None), reject(reason, None));
            for err in [
                client.check_validity().expect_err("check reject"),
                client
                    .authorize_and_meter()
                    .map(|_| ())
                    .expect_err("meter reject"),
            ] {
                assert_eq!(err.reason(), Some(reason));
                match &err {
                    LicenseError::StartupFailed(f) | LicenseError::InferenceDenied(f) => {
                        assert!(!f.message.trim().is_empty(), "{reason:?}: {err:?}")
                    }
                    LicenseError::Config(_) => panic!("unexpected config error"),
                }
            }
        }
    }

    #[test]
    fn transport_fail_is_temporary_failure_in_both_contexts() {
        let client = MockLicenseClient::new(
            MockOutcome::TransportFail {
                message: "connection refused".into(),
            },
            MockOutcome::TransportFail {
                message: "read timed out".into(),
            },
        );
        match client.check_validity() {
            Err(LicenseError::StartupFailed(f)) => {
                assert_eq!(f.reason, LicenseFailureReason::TemporaryFailure);
                assert_eq!(f.message, "connection refused");
            }
            other => panic!("expected StartupFailed, got {other:?}"),
        }
        match client.authorize_and_meter() {
            Err(LicenseError::InferenceDenied(f)) => {
                assert_eq!(f.reason, LicenseFailureReason::TemporaryFailure);
                assert_eq!(f.message, "read timed out");
            }
            other => panic!("expected InferenceDenied, got {other:?}"),
        }
    }

    #[test]
    fn check_validity_success_returns_allowed_result() {
        let client = MockLicenseClient::new(
            MockOutcome::Success {
                message: Some("ok".into()),
            },
            MockOutcome::Success { message: None },
        );

        let result = client.check_validity().expect("check should succeed");
        assert!(result.allowed);
        assert_eq!(result.message.as_deref(), Some("ok"));
    }

    #[test]
    fn authorize_and_meter_success_returns_allowed_result() {
        let client = MockLicenseClient::new(
            MockOutcome::Success { message: None },
            MockOutcome::Success {
                message: Some("metered".into()),
            },
        );

        let result = client.authorize_and_meter().expect("meter should succeed");
        assert!(result.allowed);
        assert_eq!(result.message.as_deref(), Some("metered"));
    }

    #[test]
    fn check_reject_maps_to_startup_failed() {
        let client = MockLicenseClient::new(
            reject(
                LicenseFailureReason::LicenseSuspended,
                Some("license suspended"),
            ),
            MockOutcome::Success { message: None },
        );

        let err = client.check_validity().expect_err("reject must be Err");
        assert!(
            matches!(&err, LicenseError::StartupFailed(f) if f.message.contains("license suspended")),
            "check reject must be StartupFailed: {err:?}"
        );
        assert!(
            err.to_string().contains("license_suspended"),
            "Display must include reason code: {err}"
        );
    }

    #[test]
    fn meter_reject_maps_to_inference_denied() {
        let client = MockLicenseClient::new(
            MockOutcome::Success { message: None },
            reject(
                LicenseFailureReason::MonthlyLimitReached,
                Some("quota exceeded"),
            ),
        );

        let err = client
            .authorize_and_meter()
            .expect_err("reject must be Err");
        assert!(
            matches!(&err, LicenseError::InferenceDenied(f) if f.message.contains("quota exceeded")),
            "meter reject must be InferenceDenied: {err:?}"
        );
        assert!(
            err.to_string().contains("monthly_limit_reached"),
            "Display must include reason code: {err}"
        );
    }

    #[test]
    fn check_transport_fail_maps_to_startup_failed() {
        let client = MockLicenseClient::new(
            MockOutcome::TransportFail {
                message: "connection timed out".into(),
            },
            MockOutcome::Success { message: None },
        );

        let err = client
            .check_validity()
            .expect_err("transport fail must be Err");
        assert!(
            matches!(&err, LicenseError::StartupFailed(f) if f.message.contains("connection timed out")),
            "check transport must be StartupFailed: {err:?}"
        );
        assert!(
            err.to_string().contains("startup failed"),
            "Display must include startup category: {err}"
        );
    }

    #[test]
    fn meter_transport_fail_maps_to_inference_denied() {
        let client = MockLicenseClient::new(
            MockOutcome::Success { message: None },
            MockOutcome::TransportFail {
                message: "dns failure".into(),
            },
        );

        let err = client
            .authorize_and_meter()
            .expect_err("transport fail must be Err");
        assert!(
            matches!(&err, LicenseError::InferenceDenied(f) if f.message.contains("dns failure")),
            "meter transport must be InferenceDenied: {err:?}"
        );
        assert!(
            err.to_string().contains("inference denied"),
            "Display must include inference category: {err}"
        );
    }

    #[test]
    fn mock_does_not_perform_network_io() {
        // Success/reject/transport are fully in-process; constructing and calling
        // with absurd URLs is unnecessary — outcomes never touch the network.
        let client = MockLicenseClient::new(
            MockOutcome::TransportFail {
                message: "simulated unreachable".into(),
            },
            reject(LicenseFailureReason::LicenseInvalid, Some("simulated deny")),
        );

        assert!(matches!(
            client.check_validity(),
            Err(LicenseError::StartupFailed(_))
        ));
        assert!(matches!(
            client.authorize_and_meter(),
            Err(LicenseError::InferenceDenied(_))
        ));
    }

    #[test]
    fn trait_object_supports_both_operations() {
        let client: Box<dyn LicenseClient> = Box::new(MockLicenseClient::new(
            MockOutcome::Success { message: None },
            MockOutcome::Success { message: None },
        ));

        assert!(client.check_validity().unwrap().allowed);
        assert!(client.authorize_and_meter().unwrap().allowed);
    }
}
