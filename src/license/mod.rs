//! License client domain: errors, config, HTTP port, and process-wide gate.
//!
//! Config loading lives in [`config`]; the [`client`] port is mockable;
//! [`http`] provides the blocking reqwest adapter. Gate lands in a later task.

mod client;
mod config;
mod http;
mod types;

pub use client::{LicenseClient, MockLicenseClient, MockOutcome};
pub use config::{LicenseConfig, SecretString};
pub use http::ReqwestLicenseClient;
pub use types::{LicenseCheckResult, LicenseError, LicenseMeterResult};
