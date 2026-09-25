//! HTTP API adapter layer (library-first).
//!
//! Task 1.1 exposes this module and the `holter-http-api` binary skeleton.
//! Config, routes, handlers, and license-gated listen land in later tasks.

pub mod config;
pub mod error;
pub mod response;

pub use config::{HttpConfig, HttpConfigError};
pub use error::{ErrorBody, ErrorDetail, HttpError};
pub use response::{AnalyzeJsonBody, AnalyzeSummaryJson, ResponseCodec};

/// Returns true when the HTTP module is linked and callable from the binary.
///
/// Used by the startup skeleton to prove library ↔ binary wiring without
/// binding a port (listen wiring is task 4.1).
pub fn module_ready() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn http_module_is_ready() {
        assert!(module_ready());
    }
}
