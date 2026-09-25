//! HTTP request handlers (health, analyze).

pub mod health;

pub use health::{HealthBody, HealthHandler};
