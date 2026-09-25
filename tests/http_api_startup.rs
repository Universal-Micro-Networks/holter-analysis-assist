//! http-api: HTTP binary entry starts under Tokio and exits with identifiable status.
//!
//! Full listen / license gate / routes are covered by `http_api_listen.rs` (task 4.1).
//! This smoke checks that missing config fails closed (non-zero) without panicking.

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn holter_http_api_missing_config_exits_nonzero_identifiably() {
    let missing = "/tmp/holter-http-api-startup-missing-config.ini";
    let _ = std::fs::remove_file(missing);

    Command::cargo_bin("holter-http-api")
        .expect("holter-http-api binary must be declared")
        .args(["--config", missing])
        .assert()
        .failure()
        .stderr(
            predicate::str::contains("http config")
                .or(predicate::str::contains("failed to read"))
                .or(predicate::str::contains("error:")),
        );
}
