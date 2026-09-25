//! http-api task 1.1: Cargo.toml declares Axum 0.7.x, Tokio multi-thread, tower-http limit/timeout.
//!
//! License gate, routes, and packaging are deferred; this locks HTTP runtime dependency
//! declarations required by design (MSRV 1.74).

use std::fs;
use std::path::PathBuf;

fn cargo_toml() -> String {
    let mut path = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    path.push("Cargo.toml");
    fs::read_to_string(&path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Extract the `[dependencies]` table body (until the next `[...]` section).
fn dependencies_section(toml: &str) -> &str {
    let start = toml
        .find("[dependencies]")
        .expect("Cargo.toml must have [dependencies]");
    let after = &toml[start + "[dependencies]".len()..];
    let end = after.find("\n[").unwrap_or(after.len());
    &after[..end]
}

#[test]
fn cargo_toml_declares_axum_07() {
    let toml = cargo_toml();
    let deps = dependencies_section(&toml);
    let axum_line = deps
        .lines()
        .find(|l| l.trim_start().starts_with("axum"))
        .expect("[dependencies] must declare axum for HTTP server");
    assert!(
        axum_line.contains("0.7"),
        "axum must be 0.7.x (MSRV 1.74; 0.8 not adopted); got: {axum_line}"
    );
}

#[test]
fn cargo_toml_declares_tokio_multi_thread() {
    let toml = cargo_toml();
    let deps = dependencies_section(&toml);
    assert!(
        deps.lines().any(|l| l.trim_start().starts_with("tokio")),
        "[dependencies] must declare tokio for HTTP runtime"
    );
    assert!(
        deps.contains("rt-multi-thread") || deps.contains("\"full\""),
        "tokio must enable multi-thread runtime (rt-multi-thread); deps section:\n{deps}"
    );
}

#[test]
fn cargo_toml_declares_tower_http_limit_timeout() {
    let toml = cargo_toml();
    let deps = dependencies_section(&toml);
    assert!(
        deps.lines()
            .any(|l| l.trim_start().starts_with("tower-http")),
        "[dependencies] must declare tower-http for body limit / timeout"
    );
    assert!(
        deps.contains("limit") && deps.contains("timeout"),
        "tower-http must enable features limit and timeout; deps section:\n{deps}"
    );
}

#[test]
fn cargo_toml_declares_holter_http_api_bin() {
    let toml = cargo_toml();
    assert!(
        toml.contains("name = \"holter-http-api\"") || toml.contains("name=\"holter-http-api\""),
        "Cargo.toml must declare [[bin]] name = \"holter-http-api\" separate from CLI"
    );
    assert!(
        toml.contains("holter_http_api.rs") || toml.contains("src/bin/holter_http_api"),
        "holter-http-api bin path must point at src/bin/holter_http_api.rs"
    );
}
