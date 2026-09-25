//! License client domain: errors, config, HTTP port, and process-wide gate.
//!
//! Config loading lives in [`config`]; the [`client`] port is mockable; HTTP /
//! gate land in later tasks.

mod client;
mod config;
mod types;

pub use client::{LicenseClient, MockLicenseClient, MockOutcome};
pub use config::{LicenseConfig, SecretString};
pub use types::{LicenseCheckResult, LicenseError, LicenseMeterResult};
