//! HTTP API adapter layer (library-first).
//!
//! Binary `holter-http-api` calls [`run`] for the fail-closed startup sequence.

pub mod assets;
pub mod config;
pub mod error;
pub mod handlers;
pub mod response;
pub mod routes;
pub mod startup;
pub mod state;

pub use config::{HttpConfig, HttpConfigError};
pub use error::{ErrorBody, ErrorDetail, HttpError};
pub use handlers::{static_ui_router, AnalyzeHandler, HealthBody, HealthHandler, StaticUiHandler};
pub use response::{AnalyzeJsonBody, AnalyzeSummaryJson, ResponseCodec};
pub use routes::build_router;
pub use startup::{
    install_and_ensure_startup_licensed, run, run_blocking, serve, StartupError,
};
pub use state::{AppState, SharedModel};

/// Returns true when the HTTP module is linked and callable from the binary.
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
