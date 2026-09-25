//! license-client task 1.1: Cargo.toml declares reqwest 0.12 + rust-ini (ini).
//!
//! Runtime license modules are deferred to later tasks; this only locks dependency
//! declarations required by design (blocking HTTP + ini parse, MSRV 1.74).

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
fn cargo_toml_declares_reqwest_012_blocking_json_rustls() {
    let toml = cargo_toml();
    let deps = dependencies_section(&toml);
    assert!(
        deps.contains("reqwest"),
        "[dependencies] must declare reqwest for license HTTP client"
    );
    // Accept either inline table or multi-line form; require 0.12 line and features.
    let reqwest_block = deps
        .lines()
        .find(|l| l.trim_start().starts_with("reqwest"))
        .expect("reqwest dependency line");
    assert!(
        reqwest_block.contains("0.12"),
        "reqwest must be 0.12.x (MSRV 1.74); got: {reqwest_block}"
    );
    assert!(
        deps.contains("blocking") && deps.contains("json") && deps.contains("rustls-tls"),
        "reqwest must enable features blocking, json, rustls-tls; deps section:\n{deps}"
    );
}

#[test]
fn cargo_toml_declares_ini_rust_ini_021() {
    let toml = cargo_toml();
    let deps = dependencies_section(&toml);
    let ini_line = deps
        .lines()
        .find(|l| {
            let t = l.trim_start();
            t.starts_with("ini ") || t.starts_with("ini=") || t.starts_with("ini =")
        })
        .expect("[dependencies] must declare ini (rust-ini) for license config");
    assert!(
        ini_line.contains("0.21") && ini_line.contains("rust-ini"),
        "ini must be package rust-ini 0.21.x (not crates.io 'ini'); got: {ini_line}"
    );
}
