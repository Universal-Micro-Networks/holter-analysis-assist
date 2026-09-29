//! Process-wide license gate: startup validity and per-inference authorize+meter.
//!
//! Install once at process start (`install` / `global` / `try_global`).
//! Startup uses [`LicenseClient::check_validity`] only; inference uses
//! [`LicenseClient::authorize_and_meter`] only. Fail-closed: no offline bypass.
//! Uninstalled gate on a metering path must deny inference.

use std::sync::Mutex;

use super::client::LicenseClient;
use super::types::{
    LicenseCheckResult, LicenseError, LicenseFailure, LicenseFailureReason, LicenseMeterResult,
};

/// Process-wide license gate wrapping a [`LicenseClient`].
pub struct LicenseGate {
    client: Box<dyn LicenseClient>,
    /// Last startup validity failure; while set, inference is denied without metering.
    startup_failure: Mutex<Option<LicenseFailure>>,
}

static GLOBAL_GATE: Mutex<Option<&'static LicenseGate>> = Mutex::new(None);

/// Serializes tests that mutate the process-wide gate (shared across modules).
#[cfg(test)]
pub(crate) static GLOBAL_TEST_LOCK: Mutex<()> = Mutex::new(());

impl LicenseGate {
    /// Build a gate around an owned client (mock or HTTP adapter).
    pub fn new(client: impl LicenseClient + 'static) -> Self {
        Self {
            client: Box::new(client),
            startup_failure: Mutex::new(None),
        }
    }

    /// Register the process-wide gate once (CLI / HTTP main).
    ///
    /// Returns [`LicenseError::Config`] if a gate is already installed.
    pub fn install(gate: Self) -> Result<(), LicenseError> {
        let mut slot = GLOBAL_GATE
            .lock()
            .map_err(|_| LicenseError::Config("license gate lock poisoned".into()))?;
        if slot.is_some() {
            return Err(LicenseError::Config(
                "license gate already installed".into(),
            ));
        }
        *slot = Some(Box::leak(Box::new(gate)));
        Ok(())
    }

    /// Process-wide gate; panics if not installed. Prefer [`try_global`] on analyze paths.
    pub fn global() -> &'static Self {
        Self::try_global().expect("LicenseGate::install must be called before LicenseGate::global")
    }

    /// Process-wide gate if installed.
    pub fn try_global() -> Option<&'static Self> {
        match GLOBAL_GATE.lock() {
            Ok(slot) => *slot,
            Err(poisoned) => *poisoned.into_inner(),
        }
    }

    /// Startup gate: validity check only (no meter). Ok only when allowed.
    ///
    /// The outcome is remembered: a `StartupFailed` result blocks inference until a
    /// later call succeeds (see [`Self::startup_failure`]).
    pub fn ensure_startup_licensed(&self) -> Result<(), LicenseError> {
        let outcome = self.check_validity_once();
        match &outcome {
            Ok(()) => self.set_startup_failure(None),
            Err(LicenseError::StartupFailed(failure)) => {
                self.set_startup_failure(Some(failure.clone()))
            }
            Err(_) => {}
        }
        outcome
    }

    /// Failure of the latest startup validity check, if it did not pass.
    pub fn startup_failure(&self) -> Option<LicenseFailure> {
        match self.startup_failure.lock() {
            Ok(slot) => slot.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
    }

    fn set_startup_failure(&self, failure: Option<LicenseFailure>) {
        match self.startup_failure.lock() {
            Ok(mut slot) => *slot = failure,
            Err(poisoned) => *poisoned.into_inner() = failure,
        }
    }

    fn check_validity_once(&self) -> Result<(), LicenseError> {
        let result = self.client.check_validity()?;
        if !result.allowed {
            return Err(LicenseError::StartupFailed(LicenseFailure::new(
                LicenseFailureReason::UnexpectedResponse,
                result
                    .message
                    .unwrap_or_else(|| "validity check denied".into()),
            )));
        }
        eprintln!("{}", verified_log_line(&result));
        Ok(())
    }

    /// Inference gate: authorize + meter. Ok only when allowed.
    ///
    /// Denied without contacting the server while the startup check has not passed.
    pub fn ensure_inference_allowed(&self) -> Result<(), LicenseError> {
        if let Some(failure) = self.startup_failure() {
            return Err(LicenseError::InferenceDenied(failure));
        }
        let result = self.client.authorize_and_meter()?;
        if !result.allowed {
            return Err(LicenseError::InferenceDenied(LicenseFailure::new(
                LicenseFailureReason::UnexpectedResponse,
                result
                    .message
                    .unwrap_or_else(|| "authorize and meter denied".into()),
            )));
        }
        eprintln!("{}", metered_log_line(&result));
        Ok(())
    }
}

/// Stderr line for a successful startup validity check.
fn verified_log_line(result: &LicenseCheckResult) -> String {
    let limit = match result.monthly_limit {
        Some(0) => "unlimited".to_string(),
        Some(n) => n.to_string(),
        None => "-".to_string(),
    };
    format!(
        "license: verified status={} monthly_limit={limit}",
        result.status.as_deref().unwrap_or("-")
    )
}

/// Stderr line for a successful authorize-and-meter call.
fn metered_log_line(result: &LicenseMeterResult) -> String {
    match &result.usage {
        Some(usage) if usage.is_unlimited() => {
            format!(
                "license: usage recorded used={} monthly_limit=unlimited",
                usage.used
            )
        }
        Some(usage) => format!(
            "license: usage recorded used={} monthly_limit={} remaining={}",
            usage.used,
            usage.monthly_limit,
            usage
                .remaining
                .map_or_else(|| "-".to_string(), |n| n.to_string())
        ),
        None => "license: usage recorded".to_string(),
    }
}

#[cfg(test)]
impl LicenseGate {
    /// Clear the process-wide slot (tests only; serializes via [`GLOBAL_TEST_LOCK`]).
    pub(crate) fn clear_for_test() {
        match GLOBAL_GATE.lock() {
            Ok(mut slot) => *slot = None,
            Err(poisoned) => *poisoned.into_inner() = None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::LicenseGate;
    use crate::license::client::{LicenseClient, MockLicenseClient, MockOutcome};
    use crate::license::types::{
        LicenseCheckResult, LicenseError, LicenseFailure, LicenseFailureReason, LicenseMeterResult,
    };
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;

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

    fn with_clean_global(f: impl FnOnce()) {
        let _guard = super::GLOBAL_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        LicenseGate::clear_for_test();
        f();
        LicenseGate::clear_for_test();
    }

    /// Counts check vs meter calls to prove startup/inference separation.
    struct CountingClient {
        inner: MockLicenseClient,
        check_calls: Arc<AtomicUsize>,
        meter_calls: Arc<AtomicUsize>,
    }

    impl CountingClient {
        fn new(
            check: MockOutcome,
            meter: MockOutcome,
            check_calls: Arc<AtomicUsize>,
            meter_calls: Arc<AtomicUsize>,
        ) -> Self {
            Self {
                inner: MockLicenseClient::new(check, meter),
                check_calls,
                meter_calls,
            }
        }
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

    /// Client that returns Ok with `allowed: false` (Gate must still fail-closed).
    struct AllowedFalseClient;

    impl LicenseClient for AllowedFalseClient {
        fn check_validity(&self) -> Result<LicenseCheckResult, LicenseError> {
            Ok(LicenseCheckResult {
                allowed: false,
                message: Some("not allowed".into()),
                ..Default::default()
            })
        }

        fn authorize_and_meter(&self) -> Result<LicenseMeterResult, LicenseError> {
            Ok(LicenseMeterResult {
                allowed: false,
                message: Some("not allowed".into()),
                ..Default::default()
            })
        }
    }

    #[test]
    fn startup_success_returns_ok() {
        let gate = LicenseGate::new(MockLicenseClient::new(
            MockOutcome::Success {
                message: Some("ok".into()),
            },
            MockOutcome::Success { message: None },
        ));
        gate.ensure_startup_licensed()
            .expect("startup should succeed when check allows");
    }

    #[test]
    fn startup_reject_returns_startup_failed_not_ok() {
        let gate = LicenseGate::new(MockLicenseClient::new(
            reject(
                LicenseFailureReason::LicenseInvalid,
                Some("license expired"),
            ),
            MockOutcome::Success { message: None },
        ));
        let err = gate
            .ensure_startup_licensed()
            .expect_err("startup reject must not return Ok");
        assert!(
            matches!(&err, LicenseError::StartupFailed(f) if f.message.contains("license expired")),
            "must be StartupFailed: {err:?}"
        );
        assert!(
            err.to_string().contains("startup failed"),
            "Display must include startup category: {err}"
        );
    }

    #[test]
    fn startup_reject_propagates_reason_unchanged() {
        for reason in ALL_REASONS {
            let gate = LicenseGate::new(MockLicenseClient::new(
                reject(reason, Some("rejected")),
                MockOutcome::Success { message: None },
            ));
            let err = gate
                .ensure_startup_licensed()
                .expect_err("startup reject must not return Ok");
            assert_eq!(
                err,
                LicenseError::StartupFailed(LicenseFailure::new(reason, "rejected")),
                "{reason:?}"
            );
        }
    }

    #[test]
    fn startup_transport_fail_returns_startup_failed() {
        let gate = LicenseGate::new(MockLicenseClient::new(
            MockOutcome::TransportFail {
                message: "connection timed out".into(),
            },
            MockOutcome::Success { message: None },
        ));
        let err = gate
            .ensure_startup_licensed()
            .expect_err("startup transport must not return Ok");
        assert_eq!(
            err,
            LicenseError::StartupFailed(LicenseFailure::new(
                LicenseFailureReason::TemporaryFailure,
                "connection timed out",
            ))
        );
    }

    #[test]
    fn startup_allowed_false_is_startup_failed_unexpected_response() {
        let gate = LicenseGate::new(AllowedFalseClient);
        let err = gate
            .ensure_startup_licensed()
            .expect_err("allowed:false must not return Ok");
        assert!(
            matches!(
                &err,
                LicenseError::StartupFailed(f)
                    if f.reason == LicenseFailureReason::UnexpectedResponse
                        && f.message == "not allowed"
            ),
            "{err:?}"
        );
    }

    #[test]
    fn startup_calls_check_validity_only_not_meter() {
        let check_calls = Arc::new(AtomicUsize::new(0));
        let meter_calls = Arc::new(AtomicUsize::new(0));
        let gate = LicenseGate::new(CountingClient::new(
            MockOutcome::Success { message: None },
            MockOutcome::Success { message: None },
            Arc::clone(&check_calls),
            Arc::clone(&meter_calls),
        ));
        gate.ensure_startup_licensed().unwrap();
        assert_eq!(
            check_calls.load(Ordering::SeqCst),
            1,
            "startup must call check_validity once"
        );
        assert_eq!(
            meter_calls.load(Ordering::SeqCst),
            0,
            "startup must not call authorize_and_meter"
        );
    }

    /// Validity check fails until `reachable` is set; meter always succeeds.
    struct RecoveringClient {
        reachable: Arc<std::sync::atomic::AtomicBool>,
        meter_calls: Arc<AtomicUsize>,
    }

    impl LicenseClient for RecoveringClient {
        fn check_validity(&self) -> Result<LicenseCheckResult, LicenseError> {
            if self.reachable.load(Ordering::SeqCst) {
                Ok(LicenseCheckResult {
                    allowed: true,
                    ..Default::default()
                })
            } else {
                Err(LicenseError::StartupFailed(LicenseFailure::new(
                    LicenseFailureReason::TemporaryFailure,
                    "license server unreachable",
                )))
            }
        }

        fn authorize_and_meter(&self) -> Result<LicenseMeterResult, LicenseError> {
            self.meter_calls.fetch_add(1, Ordering::SeqCst);
            Ok(LicenseMeterResult {
                allowed: true,
                ..Default::default()
            })
        }
    }

    #[test]
    fn failed_startup_blocks_inference_without_metering_until_recheck_passes() {
        let reachable = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let meter_calls = Arc::new(AtomicUsize::new(0));
        let gate = LicenseGate::new(RecoveringClient {
            reachable: Arc::clone(&reachable),
            meter_calls: Arc::clone(&meter_calls),
        });

        assert!(gate.ensure_startup_licensed().is_err());
        let failure = gate
            .startup_failure()
            .expect("startup failure is remembered");
        assert_eq!(failure.reason, LicenseFailureReason::TemporaryFailure);

        let err = gate
            .ensure_inference_allowed()
            .expect_err("inference must stay denied after a failed startup check");
        assert_eq!(err, LicenseError::InferenceDenied(failure));
        assert_eq!(
            meter_calls.load(Ordering::SeqCst),
            0,
            "denied inference must not be metered"
        );

        reachable.store(true, Ordering::SeqCst);
        gate.ensure_startup_licensed()
            .expect("recheck succeeds once the server is back");
        assert_eq!(gate.startup_failure(), None);
        gate.ensure_inference_allowed()
            .expect("inference is allowed after a passing recheck");
        assert_eq!(meter_calls.load(Ordering::SeqCst), 1);
    }

    #[test]
    fn inference_success_returns_ok() {
        let gate = LicenseGate::new(MockLicenseClient::new(
            MockOutcome::Success { message: None },
            MockOutcome::Success {
                message: Some("metered".into()),
            },
        ));
        gate.ensure_inference_allowed()
            .expect("inference should succeed when meter allows");
    }

    #[test]
    fn inference_reject_returns_inference_denied_not_ok() {
        let gate = LicenseGate::new(MockLicenseClient::new(
            MockOutcome::Success { message: None },
            reject(
                LicenseFailureReason::MonthlyLimitReached,
                Some("quota exceeded"),
            ),
        ));
        let err = gate
            .ensure_inference_allowed()
            .expect_err("inference reject must not return Ok");
        assert!(
            matches!(&err, LicenseError::InferenceDenied(f) if f.message.contains("quota exceeded")),
            "must be InferenceDenied: {err:?}"
        );
        assert!(
            err.to_string().contains("inference denied"),
            "Display must include inference category: {err}"
        );
    }

    #[test]
    fn inference_reject_propagates_reason_unchanged() {
        for reason in ALL_REASONS {
            let gate = LicenseGate::new(MockLicenseClient::new(
                MockOutcome::Success { message: None },
                reject(reason, Some("rejected")),
            ));
            let err = gate
                .ensure_inference_allowed()
                .expect_err("inference reject must not return Ok");
            assert_eq!(
                err,
                LicenseError::InferenceDenied(LicenseFailure::new(reason, "rejected")),
                "{reason:?}"
            );
        }
    }

    #[test]
    fn inference_transport_fail_returns_inference_denied() {
        let gate = LicenseGate::new(MockLicenseClient::new(
            MockOutcome::Success { message: None },
            MockOutcome::TransportFail {
                message: "dns failure".into(),
            },
        ));
        let err = gate
            .ensure_inference_allowed()
            .expect_err("inference transport must not return Ok");
        assert_eq!(
            err,
            LicenseError::InferenceDenied(LicenseFailure::new(
                LicenseFailureReason::TemporaryFailure,
                "dns failure",
            ))
        );
    }

    #[test]
    fn inference_allowed_false_is_inference_denied_unexpected_response() {
        let gate = LicenseGate::new(AllowedFalseClient);
        let err = gate
            .ensure_inference_allowed()
            .expect_err("allowed:false must not return Ok");
        assert!(
            matches!(
                &err,
                LicenseError::InferenceDenied(f)
                    if f.reason == LicenseFailureReason::UnexpectedResponse
                        && f.message == "not allowed"
            ),
            "{err:?}"
        );
    }

    #[test]
    fn allowed_false_without_message_still_has_non_empty_summary() {
        struct SilentDenyClient;
        impl LicenseClient for SilentDenyClient {
            fn check_validity(&self) -> Result<LicenseCheckResult, LicenseError> {
                Ok(LicenseCheckResult::default())
            }
            fn authorize_and_meter(&self) -> Result<LicenseMeterResult, LicenseError> {
                Ok(LicenseMeterResult::default())
            }
        }
        let gate = LicenseGate::new(SilentDenyClient);
        for err in [
            gate.ensure_startup_licensed().expect_err("startup"),
            gate.ensure_inference_allowed().expect_err("inference"),
        ] {
            assert_eq!(err.reason(), Some(LicenseFailureReason::UnexpectedResponse));
            match &err {
                LicenseError::StartupFailed(f) | LicenseError::InferenceDenied(f) => {
                    assert!(!f.message.trim().is_empty(), "{err:?}")
                }
                LicenseError::Config(_) => panic!("unexpected config error"),
            }
        }
    }

    #[test]
    fn rejected_inference_still_counts_one_meter_call_and_no_check() {
        let check_calls = Arc::new(AtomicUsize::new(0));
        let meter_calls = Arc::new(AtomicUsize::new(0));
        let gate = LicenseGate::new(CountingClient::new(
            MockOutcome::Success { message: None },
            reject(LicenseFailureReason::RateLimited, None),
            Arc::clone(&check_calls),
            Arc::clone(&meter_calls),
        ));
        let err = gate.ensure_inference_allowed().expect_err("rejected");
        assert_eq!(err.reason(), Some(LicenseFailureReason::RateLimited));
        assert_eq!(meter_calls.load(Ordering::SeqCst), 1);
        assert_eq!(check_calls.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn inference_calls_authorize_and_meter_only_not_check() {
        let check_calls = Arc::new(AtomicUsize::new(0));
        let meter_calls = Arc::new(AtomicUsize::new(0));
        let gate = LicenseGate::new(CountingClient::new(
            MockOutcome::Success { message: None },
            MockOutcome::Success { message: None },
            Arc::clone(&check_calls),
            Arc::clone(&meter_calls),
        ));
        gate.ensure_inference_allowed().unwrap();
        assert_eq!(
            meter_calls.load(Ordering::SeqCst),
            1,
            "inference must call authorize_and_meter once"
        );
        assert_eq!(
            check_calls.load(Ordering::SeqCst),
            0,
            "inference must not call check_validity"
        );
    }

    #[test]
    fn try_global_none_when_not_installed_and_inference_path_fail_closed() {
        with_clean_global(|| {
            assert!(
                LicenseGate::try_global().is_none(),
                "gate must be uninstalled for this test"
            );
            // Contract for metering paths (analyze entry): fail-closed when unset.
            let denied = LicenseGate::try_global()
                .map(|g| g.ensure_inference_allowed())
                .unwrap_or_else(|| {
                    Err(LicenseError::InferenceDenied(LicenseFailure::new(
                        LicenseFailureReason::GateNotInstalled,
                        "license gate not installed",
                    )))
                });
            assert!(
                matches!(denied, Err(LicenseError::InferenceDenied(_))),
                "uninstalled inference path must fail-closed: {denied:?}"
            );
        });
    }

    #[test]
    fn install_then_global_and_try_global_return_same_gate() {
        with_clean_global(|| {
            let gate = LicenseGate::new(MockLicenseClient::new(
                MockOutcome::Success { message: None },
                MockOutcome::Success { message: None },
            ));
            LicenseGate::install(gate).expect("install should succeed once");
            let g1 = LicenseGate::try_global().expect("try_global after install");
            let g2 = LicenseGate::global();
            assert!(
                std::ptr::eq(g1, g2),
                "global and try_global must refer to the same instance"
            );
            g1.ensure_startup_licensed().unwrap();
            g1.ensure_inference_allowed().unwrap();
        });
    }

    #[test]
    fn double_install_returns_err() {
        with_clean_global(|| {
            let a = LicenseGate::new(MockLicenseClient::new(
                MockOutcome::Success { message: None },
                MockOutcome::Success { message: None },
            ));
            let b = LicenseGate::new(MockLicenseClient::new(
                MockOutcome::Success { message: None },
                MockOutcome::Success { message: None },
            ));
            LicenseGate::install(a).expect("first install ok");
            let err = LicenseGate::install(b).expect_err("second install must fail");
            assert!(
                matches!(err, LicenseError::Config(_)),
                "double install must be Config Err: {err:?}"
            );
        });
    }

    #[test]
    fn no_offline_bypass_on_check_or_meter_failure() {
        let startup = LicenseGate::new(MockLicenseClient::new(
            MockOutcome::TransportFail {
                message: "unreachable".into(),
            },
            MockOutcome::Success { message: None },
        ));
        assert!(
            startup.ensure_startup_licensed().is_err(),
            "offline/unreachable must not bypass startup"
        );

        let inference = LicenseGate::new(MockLicenseClient::new(
            MockOutcome::Success { message: None },
            MockOutcome::TransportFail {
                message: "unreachable".into(),
            },
        ));
        assert!(
            inference.ensure_inference_allowed().is_err(),
            "offline/unreachable must not bypass inference"
        );
    }

    #[test]
    fn verified_log_line_reports_status_and_limit() {
        let line = super::verified_log_line(&LicenseCheckResult {
            allowed: true,
            status: Some("active".into()),
            monthly_limit: Some(0),
            message: None,
        });
        assert_eq!(
            line,
            "license: verified status=active monthly_limit=unlimited"
        );

        let line = super::verified_log_line(&LicenseCheckResult {
            allowed: true,
            monthly_limit: Some(500),
            ..Default::default()
        });
        assert_eq!(line, "license: verified status=- monthly_limit=500");
    }

    #[test]
    fn metered_log_line_reports_usage_counters() {
        use crate::license::types::UsageSnapshot;
        let metered = |usage| {
            super::metered_log_line(&LicenseMeterResult {
                allowed: true,
                usage,
                message: None,
            })
        };
        assert_eq!(
            metered(Some(UsageSnapshot {
                used: 3,
                monthly_limit: 100,
                remaining: Some(97),
            })),
            "license: usage recorded used=3 monthly_limit=100 remaining=97"
        );
        assert_eq!(
            metered(Some(UsageSnapshot {
                used: 7,
                monthly_limit: 0,
                remaining: None,
            })),
            "license: usage recorded used=7 monthly_limit=unlimited"
        );
        assert_eq!(metered(None), "license: usage recorded");
    }
}
