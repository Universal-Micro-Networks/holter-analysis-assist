//! Swappable license-server port and a network-free test mock.
//!
//! [`LicenseClient`] is the boundary Gate (and tests) depend on.
//! [`MockLicenseClient`] scripts success / deny / transport failure without
//! reaching an external server. Transport failures map by operation:
//! check → [`LicenseError::StartupFailed`], meter → [`LicenseError::InferenceDenied`].

use super::types::{LicenseCheckResult, LicenseError, LicenseMeterResult};

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
    /// Server responds with an explicit deny (`allowed: false`).
    Deny { message: Option<String> },
    /// Transport / timeout / communication failure (no successful response).
    TransportFail { message: String },
}

/// Test double that reproduces success / deny / transport without network I/O.
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
        match &self.check {
            MockOutcome::Success { message } => Ok(LicenseCheckResult {
                allowed: true,
                message: message.clone(),
            }),
            MockOutcome::Deny { message } => Err(LicenseError::StartupFailed(
                message
                    .clone()
                    .unwrap_or_else(|| "validity check denied".into()),
            )),
            MockOutcome::TransportFail { message } => {
                Err(LicenseError::StartupFailed(message.clone()))
            }
        }
    }

    fn authorize_and_meter(&self) -> Result<LicenseMeterResult, LicenseError> {
        match &self.meter {
            MockOutcome::Success { message } => Ok(LicenseMeterResult {
                allowed: true,
                message: message.clone(),
            }),
            MockOutcome::Deny { message } => Err(LicenseError::InferenceDenied(
                message
                    .clone()
                    .unwrap_or_else(|| "authorize and meter denied".into()),
            )),
            MockOutcome::TransportFail { message } => {
                Err(LicenseError::InferenceDenied(message.clone()))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{LicenseClient, MockLicenseClient, MockOutcome};
    use crate::license::types::LicenseError;

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

        let result = client
            .authorize_and_meter()
            .expect("meter should succeed");
        assert!(result.allowed);
        assert_eq!(result.message.as_deref(), Some("metered"));
    }

    #[test]
    fn check_deny_maps_to_startup_failed() {
        let client = MockLicenseClient::new(
            MockOutcome::Deny {
                message: Some("license expired".into()),
            },
            MockOutcome::Success { message: None },
        );

        let err = client.check_validity().expect_err("deny must be Err");
        assert!(
            matches!(&err, LicenseError::StartupFailed(msg) if msg.contains("license expired")),
            "check deny must be StartupFailed: {err:?}"
        );
    }

    #[test]
    fn meter_deny_maps_to_inference_denied() {
        let client = MockLicenseClient::new(
            MockOutcome::Success { message: None },
            MockOutcome::Deny {
                message: Some("quota exceeded".into()),
            },
        );

        let err = client
            .authorize_and_meter()
            .expect_err("deny must be Err");
        assert!(
            matches!(&err, LicenseError::InferenceDenied(msg) if msg.contains("quota exceeded")),
            "meter deny must be InferenceDenied: {err:?}"
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
            matches!(&err, LicenseError::StartupFailed(msg) if msg.contains("connection timed out")),
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
            matches!(&err, LicenseError::InferenceDenied(msg) if msg.contains("dns failure")),
            "meter transport must be InferenceDenied: {err:?}"
        );
        assert!(
            err.to_string().contains("inference denied"),
            "Display must include inference category: {err}"
        );
    }

    #[test]
    fn mock_does_not_perform_network_io() {
        // Success/deny/transport are fully in-process; constructing and calling
        // with absurd URLs is unnecessary — outcomes never touch the network.
        let client = MockLicenseClient::new(
            MockOutcome::TransportFail {
                message: "simulated unreachable".into(),
            },
            MockOutcome::Deny {
                message: Some("simulated deny".into()),
            },
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
