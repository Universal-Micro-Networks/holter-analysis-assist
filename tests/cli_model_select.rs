//! CLI CliModelSelect integration (model-embedding task 5.2).
//!
//! Covers `--model` missing path → non-zero exit + clear message.
//! With license-client CliStartupIntegration, the process requires a reachable
//! license server before any subcommand — tests use the shared mock license
//! server (`tests/common/license_mock.rs`).
//! Embedded build without `--model` is exercised by `tools/check_cli_embed_select.sh`
//! (requires `HOLTER_EMBEDDED_MODEL_PATH` at build time).

mod common;

use assert_cmd::Command;
use common::license_mock::LicenseMockServer;
use predicates::prelude::*;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

fn write_license_ini(dir: &TempDir, mock: &LicenseMockServer) -> PathBuf {
    let path = dir.path().join("license.ini");
    std::fs::write(&path, mock.license_ini_section()).expect("write license.ini");
    path
}

fn cmd_with_mock_license(ini: &Path) -> Command {
    let mut cmd = Command::cargo_bin("holter-analysis-assist").expect("cli binary");
    cmd.args(["--license-config", ini.to_str().expect("utf8 path")]);
    cmd
}

#[test]
fn infer_window_missing_model_path_exits_nonzero_with_clear_message() {
    let mock = LicenseMockServer::start();
    let dir = TempDir::new().expect("tempdir");
    let ini = write_license_ini(&dir, &mock);

    let missing = "/tmp/holter-cli-model-select-missing.onnx";
    cmd_with_mock_license(&ini)
        .args(["infer-window", "--model", missing, "--provider", "cpu"])
        .assert()
        .failure()
        .stderr(
            predicates::str::contains("ONNX model not found")
                .and(predicates::str::contains(missing)),
        );
}

#[test]
fn analyze_ecl_missing_model_path_exits_nonzero_with_clear_message() {
    let mock = LicenseMockServer::start();
    let dir = TempDir::new().expect("tempdir");
    let ini = write_license_ini(&dir, &mock);

    let missing = "/tmp/holter-cli-model-select-missing.onnx";
    // Valid ECL filename shape so parse succeeds; model load fails before ECL I/O.
    let ecl = "/tmp/1234567890_20240101_0000_2359.ecl";
    cmd_with_mock_license(&ini)
        .args([
            "analyze-ecl",
            ecl,
            "--model",
            missing,
            "--provider",
            "cpu",
            "--output",
            "/tmp/holter-cli-model-select-out.csv",
        ])
        .assert()
        .failure()
        .stderr(
            predicates::str::contains("ONNX model not found")
                .and(predicates::str::contains(missing)),
        );
}
