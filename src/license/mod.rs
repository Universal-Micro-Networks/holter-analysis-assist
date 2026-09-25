//! License client domain: errors, config, HTTP port, and process-wide gate.
//!
//! Config loading lives in [`config`]; the [`client`] port is mockable;
//! [`http`] provides the blocking reqwest adapter; [`gate`] is the process-wide
//! startup / inference gate.

mod client;
mod config;
mod gate;
mod http;
mod types;

pub use client::{LicenseClient, MockLicenseClient, MockOutcome};
pub use config::{LicenseConfig, SecretString};
pub use gate::LicenseGate;
pub use http::ReqwestLicenseClient;
pub use types::{LicenseCheckResult, LicenseError, LicenseMeterResult};

#[cfg(test)]
pub(crate) use gate::GLOBAL_TEST_LOCK;
