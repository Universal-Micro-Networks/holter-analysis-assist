//! HTTP request handlers (health, analyze, static UI).

pub mod analyze;
pub mod health;
pub mod static_ui;

pub use analyze::AnalyzeHandler;
pub use health::{HealthBody, HealthHandler};
pub use static_ui::{static_ui_router, StaticUiHandler};
