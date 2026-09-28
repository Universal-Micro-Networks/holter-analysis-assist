//! CLI CliStartupIntegration (license-client task 4.2).
//!
//! Startup: load license.ini → install Gate → ensure_startup_licensed before any
//! subcommand. Failures exit non-zero with a startup/config message and must not
//! reach analyze. Uses the shared mock license server (`tests/common/license_mock.rs`,
//! finalized contract; no offline bypass).

mod common;

use assert_cmd::Command;
use common::license_mock::{LicenseMockServer, MockResponse, TEST_LICENSE_KEY};
use predicates::prelude::*;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn write_license_ini(dir: &TempDir, mock: &LicenseMockServer) -> PathBuf {
    let path = dir.path().join("license.ini");
    std::fs::write(&path, mock.license_ini_section()).expect("write license.ini");
    path
}

fn cmd_with_license(ini: &Path) -> Command {
    let mut cmd = Command::cargo_bin("holter-analysis-assist").expect("cli binary");
    cmd.args(["--license-config", ini.to_str().expect("utf8 path")]);
    cmd
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

    cmd_with_license(&ini)
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
        .stderr(
            predicates::str::contains("startup failed")
                .and(predicates::str::contains("license_invalid"))
                .and(predicates::str::contains(TEST_LICENSE_KEY).not()),
        );

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
        .args([
            "--license-config",
            missing_ini,
            "infer-window",
            "--model",
            "/tmp/holter-cli-license-startup-missing.onnx",
            "--provider",
            "cpu",
        ])
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
    cmd_with_license(&ini)
        .args([
            "infer-window",
            "--model",
            missing_model,
            "--provider",
            "cpu",
        ])
        .assert()
        .failure()
        .stderr(
            predicates::str::contains("ONNX model not found")
                .and(predicates::str::contains(missing_model)),
        );
    assert_eq!(mock.verify_hits(), 1, "startup must verify once");
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

    Command::cargo_bin("holter-analysis-assist")
        .expect("cli binary")
        .env("HOLTER_LICENSE_INI", ini.as_os_str())
        .args([
            "infer-window",
            "--model",
            missing_model,
            "--provider",
            "cpu",
        ])
        .assert()
        .failure()
        .stderr(
            predicates::str::contains("startup failed")
                .and(predicates::str::contains("license_suspended")),
        );
    assert_eq!(
        mock.verify_hits(),
        1,
        "env-resolved ini must reach the mock"
    );
}
