//! HTTP server binary entry (`holter-http-api`).
//!
//! Skeleton only (http-api task 1.1): proves Tokio multi-thread startup and
//! library `http` module linkage. Full sequence
//! (ini → LicenseGate::install → ensure_startup_licensed → bind) is task 4.1.

use holter_analysis_assist::http;

#[tokio::main]
async fn main() {
    let _ = http::module_ready();
    eprintln!("holter-http-api: skeleton ready (listen/routes not wired yet)");
}
