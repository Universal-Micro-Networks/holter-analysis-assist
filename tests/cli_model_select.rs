//! CLI CliModelSelect integration (model-embedding task 5.2).
//!
//! Covers `--model` missing path → non-zero exit + clear message.
//! Embedded build without `--model` is exercised by `tools/check_cli_embed_select.sh`
//! (requires `HOLTER_EMBEDDED_MODEL_PATH` at build time).

use assert_cmd::Command;
use predicates::prelude::*;

#[test]
fn infer_window_missing_model_path_exits_nonzero_with_clear_message() {
    let missing = "/tmp/holter-cli-model-select-missing.onnx";
    Command::cargo_bin("holter-analysis-assist")
        .expect("cli binary")
        .args([
            "infer-window",
            "--model",
            missing,
            "--provider",
            "cpu",
        ])
        .assert()
        .failure()
        .stderr(
            predicates::str::contains("ONNX model not found")
                .and(predicates::str::contains(missing)),
        );
}

#[test]
fn analyze_ecl_missing_model_path_exits_nonzero_with_clear_message() {
    let missing = "/tmp/holter-cli-model-select-missing.onnx";
    // Valid ECL filename shape so parse succeeds; model load fails before ECL I/O.
    let ecl = "/tmp/1234567890_20240101_0000_2359.ecl";
    Command::cargo_bin("holter-analysis-assist")
        .expect("cli binary")
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
