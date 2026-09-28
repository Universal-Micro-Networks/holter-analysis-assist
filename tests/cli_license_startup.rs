//! CLI CliStartupIntegration (license-client tasks 4.2, 9.1).
//!
//! Startup: load license.ini → install Gate → ensure_startup_licensed before any
//! subcommand. Failures exit non-zero with a startup/config message and must not
//! reach analyze. Uses the shared mock license server (`tests/common/license_mock.rs`,
//! finalized contract; no offline bypass). No output may contain a license key value.

mod common;

use assert_cmd::Command;
use common::license_mock::{LicenseMockServer, MockResponse, TEST_LICENSE_KEY};
use predicates::prelude::*;
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::process::Output;
use tempfile::TempDir;

/// Retired `api_key` value used in ini files; must never be echoed.
const RETIRED_API_KEY_VALUE: &str = "lk_fedcba9876543210fedcba9876543210";

fn write_license_ini(dir: &TempDir, mock: &LicenseMockServer) -> PathBuf {
    write_ini(dir, &mock.license_ini_section())
}

fn write_ini(dir: &TempDir, body: &str) -> PathBuf {
    let path = dir.path().join("license.ini");
    std::fs::write(&path, body).expect("write license.ini");
    path
}

fn cmd_with_license(ini: &Path) -> Command {
    let mut cmd = Command::cargo_bin("holter-analysis-assist").expect("cli binary");
    cmd.args(["--license-config", ini.to_str().expect("utf8 path")]);
    cmd
}

fn infer_window_args(model: &str) -> [&str; 5] {
    ["infer-window", "--model", model, "--provider", "cpu"]
}

fn assert_no_secrets(output: &Output, secrets: &[&str]) {
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    for secret in secrets {
        assert!(!stdout.contains(secret), "stdout leaked a key: {stdout}");
        assert!(!stderr.contains(secret), "stderr leaked a key: {stderr}");
    }
}

/// `http://127.0.0.1:<port>` with nothing listening (bound, then released).
fn unreachable_base_url() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind probe port");
    let addr = listener.local_addr().expect("probe local addr");
    drop(listener);
    format!("http://{addr}")
}

#[test]
fn startup_check_denied_exits_nonzero_with_startup_failed_and_skips_analyze() {
    let mock = LicenseMockServer::with_responses(
        MockResponse::error("license_invalid"),
        MockResponse::unlimited(),
    );
    let dir = TempDir::new().expect("tempdir");
    let ini = write_license_ini(&dir, &mock);

    let out_csv = dir.path().join("should-not-be-created.csv");
    let ecl = "/tmp/1234567890_20240101_0000_2359.ecl";

    let assert = cmd_with_license(&ini)
        .args([
            "analyze-ecl",
            ecl,
            "--model",
            "/tmp/holter-cli-license-startup-missing.onnx",
            "--provider",
            "cpu",
            "--output",
            out_csv.to_str().unwrap(),
        ])
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "license startup failed (license_invalid)",
        ));
    assert_no_secrets(assert.get_output(), &[TEST_LICENSE_KEY]);

    assert!(
        !out_csv.exists(),
        "analyze must not run / must not create output when startup license check fails"
    );
    assert_eq!(mock.verify_hits(), 1, "startup must verify once");
    assert_eq!(mock.usage_hits(), 0, "startup denial must not meter");
}

#[test]
fn missing_license_ini_exits_nonzero_before_subcommand() {
    let missing_ini = "/tmp/holter-cli-license-startup-missing-config.ini";
    let _ = std::fs::remove_file(missing_ini);

    Command::cargo_bin("holter-analysis-assist")
        .expect("cli binary")
        .args(["--license-config", missing_ini])
        .args(infer_window_args(
            "/tmp/holter-cli-license-startup-missing.onnx",
        ))
        .assert()
        .failure()
        .stderr(
            predicates::str::contains("license config")
                .or(predicates::str::contains("startup failed"))
                .or(predicates::str::contains("failed to read")),
        );
}

#[test]
fn startup_check_ok_allows_subcommand_to_run() {
    let mock = LicenseMockServer::start();
    let dir = TempDir::new().expect("tempdir");
    let ini = write_license_ini(&dir, &mock);
    let missing_model = "/tmp/holter-cli-license-startup-ok-missing.onnx";

    // License passes; infer-window proceeds and fails on missing model (proves gate did not block).
    let assert = cmd_with_license(&ini)
        .args(infer_window_args(missing_model))
        .assert()
        .failure()
        .stderr(
            predicates::str::contains("ONNX model not found")
                .and(predicates::str::contains(missing_model))
                .and(predicates::str::contains("license startup failed").not())
                .and(predicates::str::contains("license config error").not()),
        );
    assert_no_secrets(assert.get_output(), &[TEST_LICENSE_KEY]);
    assert_eq!(mock.verify_hits(), 1, "startup must verify once");
    assert_eq!(mock.usage_hits(), 0, "startup verify must not meter");
}

#[test]
fn env_holter_license_ini_is_honored() {
    // Deny via env-resolved ini: proves HOLTER_LICENSE_INI is used for startup gate
    // (without the flag). Must fail as startup, not as missing ONNX.
    let mock = LicenseMockServer::with_responses(
        MockResponse::error("license_suspended"),
        MockResponse::unlimited(),
    );
    let dir = TempDir::new().expect("tempdir");
    let ini = write_license_ini(&dir, &mock);
    let missing_model = "/tmp/holter-cli-license-env-missing.onnx";

    let assert = Command::cargo_bin("holter-analysis-assist")
        .expect("cli binary")
        .env("HOLTER_LICENSE_INI", ini.as_os_str())
        .args(infer_window_args(missing_model))
        .assert()
        .failure()
        .stderr(predicates::str::contains(
            "license startup failed (license_suspended)",
        ));
    assert_no_secrets(assert.get_output(), &[TEST_LICENSE_KEY]);
    assert_eq!(
        mock.verify_hits(),
        1,
        "env-resolved ini must reach the mock"
    );
    assert_eq!(mock.usage_hits(), 0, "startup denial must not meter");
}

#[test]
fn license_suspended_via_flag_exits_nonzero_with_reason() {
    let mock = LicenseMockServer::with_responses(
        MockResponse::error("license_suspended"),
        MockResponse::unlimited(),
    );
    let dir = TempDir::new().expect("tempdir");
    let ini = write_license_ini(&dir, &mock);

    let assert = cmd_with_license(&ini)
        .args(infer_window_args(
            "/tmp/holter-cli-license-suspended-missing.onnx",
        ))
        .assert()
        .failure()
        .stderr(
            predicates::str::contains("license startup failed (license_suspended)")
                .and(predicates::str::contains("ONNX model not found").not()),
        );
    assert_no_secrets(assert.get_output(), &[TEST_LICENSE_KEY]);
    assert_eq!(mock.verify_hits(), 1, "startup must verify once");
    assert_eq!(mock.usage_hits(), 0, "startup denial must not meter");
}

#[test]
fn unreachable_license_server_exits_nonzero_with_temporary_failure() {
    let dir = TempDir::new().expect("tempdir");
    let ini = write_ini(
        &dir,
        &format!(
            "[license]\nserver_url={}\nlicense_key={TEST_LICENSE_KEY}\ntimeout_secs=2\n",
            unreachable_base_url()
        ),
    );

    let assert = cmd_with_license(&ini)
        .args(infer_window_args(
            "/tmp/holter-cli-license-unreachable-missing.onnx",
        ))
        .timeout(std::time::Duration::from_secs(30))
        .assert()
        .failure()
        .stderr(
            predicates::str::contains("license startup failed (temporary_failure)")
                .and(predicates::str::contains("ONNX model not found").not()),
        );
    assert_no_secrets(assert.get_output(), &[TEST_LICENSE_KEY]);
}

#[test]
fn missing_license_key_is_rejected_before_contacting_server() {
    let mock = LicenseMockServer::start();
    let dir = TempDir::new().expect("tempdir");
    let ini = write_ini(
        &dir,
        &format!(
            "[license]\nserver_url={}\ntimeout_secs=2\n",
            mock.base_url()
        ),
    );

    let assert = cmd_with_license(&ini)
        .args(infer_window_args(
            "/tmp/holter-cli-license-no-key-missing.onnx",
        ))
        .assert()
        .failure()
        .stderr(
            predicates::str::contains("license config error")
                .and(predicates::str::contains("license_key"))
                .and(predicates::str::contains("ONNX model not found").not()),
        );
    assert_no_secrets(assert.get_output(), &[TEST_LICENSE_KEY]);
    assert_eq!(
        mock.verify_hits(),
        0,
        "config error must not contact the server"
    );
    assert_eq!(mock.usage_hits(), 0);
}

#[test]
fn api_key_only_is_rejected_with_rename_guidance_and_no_value() {
    let mock = LicenseMockServer::start();
    let dir = TempDir::new().expect("tempdir");
    let ini = write_ini(
        &dir,
        &format!(
            "[license]\nserver_url={}\napi_key={RETIRED_API_KEY_VALUE}\ntimeout_secs=2\n",
            mock.base_url()
        ),
    );

    let assert = cmd_with_license(&ini)
        .args(infer_window_args(
            "/tmp/holter-cli-license-api-key-missing.onnx",
        ))
        .assert()
        .failure()
        .stderr(
            predicates::str::contains("license config error")
                .and(predicates::str::contains(
                    "rename 'api_key' to 'license_key'",
                ))
                .and(predicates::str::contains("ONNX model not found").not()),
        );
    assert_no_secrets(
        assert.get_output(),
        &[RETIRED_API_KEY_VALUE, TEST_LICENSE_KEY],
    );
    assert_eq!(
        mock.verify_hits(),
        0,
        "config error must not contact the server"
    );
    assert_eq!(mock.usage_hits(), 0);
}
