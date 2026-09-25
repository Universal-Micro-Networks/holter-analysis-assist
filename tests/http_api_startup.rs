//! http-api task 1.1: HTTP binary entry exists and runs as a process skeleton.
//!
//! Full listen / license gate / routes are task 4.1; this only proves the binary
//! starts under Tokio and exits without panicking.

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn holter_http_api_binary_starts_and_exits() {
    Command::cargo_bin("holter-http-api")
        .expect("holter-http-api binary must be declared")
        .assert()
        .success()
        .stderr(predicate::str::contains("holter-http-api"));
}
