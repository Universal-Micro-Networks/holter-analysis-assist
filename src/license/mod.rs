//! License client domain: errors, config, HTTP port, and process-wide gate.
//!
//! Task 1.2 owns types only; config / client / HTTP / gate land in later tasks.

mod types;

pub use types::{LicenseCheckResult, LicenseError, LicenseMeterResult};
