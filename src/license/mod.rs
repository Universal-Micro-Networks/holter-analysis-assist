//! License client domain: errors, config, HTTP port, and process-wide gate.
//!
//! Config loading lives in [`config`]; HTTP client / gate land in later tasks.

mod config;
mod types;

pub use config::{LicenseConfig, SecretString};
pub use types::{LicenseCheckResult, LicenseError, LicenseMeterResult};
