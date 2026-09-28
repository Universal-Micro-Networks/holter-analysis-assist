//! `compare-accel` CLI subcommand (inference-acceleration task 3.7,
//! requirements 5.1, 5.5, 5.6, 6.1).
//!
//! - `--help` lists the subcommand, the analysis inference options and the
//!   comparison options (tolerance, stride, max windows, report dir, the
//!   five thresholds).
//! - Invalid values are clap argument errors (exit 2) raised before the CLI
//!   license check, so no license server is needed for them.
//! - A run prints the Markdown summary to stdout and exits 0; errors exit 1;
//!   a failed threshold exits 2.

mod common;

use assert_cmd::Command;
use common::license_mock::LicenseMockServer;
use holter_analysis_assist::accel_compare::report::{REPORT_JSON, REPORT_MARKDOWN};
use holter_analysis_assist::accel_compare::DEFAULT_REPORT_DIR;
use holter_analysis_assist::inference_options::{BatchSize, CudaTuning};
use holter_analysis_assist::phase2::{ExecutionProviderKind, InferenceOptions, Phase2Model};
use holter_analysis_assist::preprocess::EXPECTED_24H_SAMPLES_250;
use predicates::prelude::*;
use std::path::{Path, PathBuf};
use tempfile::TempDir;

/// clap usage errors exit with 2; license / comparison errors exit with 1.
const CLAP_USAGE_EXIT: i32 = 2;
const APP_FAILURE_EXIT: i32 = 1;
const THRESHOLD_FAILED_EXIT: i32 = 2;

const THRESHOLD_FLAGS: [&str; 5] = [
    "--min-rhythm-window-agreement",
    "--min-beat-match-rate",
    "--min-beat-class-agreement",
    "--max-prob-abs-diff",
    "--max-offset-samples",
];

const MARKDOWN_TITLE: &str = "# 推論高速化 比較レポート";

fn cli() -> Command {
    Command::cargo_bin("holter-analysis-assist").expect("cli binary")
}

fn help_text(args: &[&str]) -> String {
    let out = cli()
        .args(args)
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    String::from_utf8(out).expect("utf8 help")
}

/// License ini path that does not exist: reaching the license step fails with exit 1.
fn missing_license_ini(dir: &TempDir) -> PathBuf {
    dir.path().join("no-such-license.ini")
}

fn compare_args_with_missing_license(dir: &TempDir, extra: &[&str]) -> Vec<String> {
    let mut args = vec![
        "--license-config".to_string(),
        missing_license_ini(dir).display().to_string(),
        "compare-accel".to_string(),
        "1234567890_20250101_0000_0011.ecl".to_string(),
        "--report-dir".to_string(),
        dir.path().join("report").display().to_string(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    args
}

#[test]
fn top_level_help_lists_compare_accel() {
    let help = help_text(&["--help"]);
    assert!(help.contains("compare-accel"), "{help}");
}

#[test]
fn compare_accel_help_lists_all_options_and_exit_codes() {
    let help = help_text(&["compare-accel", "--help"]);
    for flag in [
        "--model",
        "--provider",
        "--batch-size",
        "--cuda-tf32",
        "--cuda-conv1d-pad-to-nc1d",
        "--cuda-graph",
        "--tolerance-samples",
        "--prob-stride",
        "--max-windows",
        "--report-dir",
    ]
    .iter()
    .chain(THRESHOLD_FLAGS.iter())
    {
        assert!(help.contains(flag), "missing {flag} in:\n{help}");
    }
    assert!(help.contains("<ECL>"), "{help}");
    assert!(
        help.contains("[default: 40]")
            && help.contains(&format!("[default: {DEFAULT_REPORT_DIR}]")),
        "defaults must be documented:\n{help}"
    );
    assert!(
        help.contains("true|false|on|off|1|0"),
        "same switch values as analyze-ecl:\n{help}"
    );
    assert!(help.contains("memory"), "prob-stride memory note:\n{help}");
    for code in ["0 =", "1 =", "2 ="] {
        assert!(
            help.contains(code),
            "exit code {code} undocumented:\n{help}"
        );
    }
}

#[test]
fn invalid_compare_values_are_argument_errors_before_license_check() {
    let dir = TempDir::new().expect("tempdir");
    let ini = missing_license_ini(&dir);
    let cases: &[(&str, &str)] = &[
        ("--prob-stride", "0"),
        ("--prob-stride", "-1"),
        ("--prob-stride", "abc"),
        ("--tolerance-samples", "-1"),
        ("--tolerance-samples", "1.5"),
        ("--max-windows", "0"),
        ("--max-windows", "x"),
        ("--batch-size", "0"),
        ("--batch-size", "257"),
        ("--cuda-graph", "yes"),
        ("--provider", "tpu"),
        ("--min-rhythm-window-agreement", "-0.1"),
        ("--min-beat-match-rate", "1.5"),
        ("--min-beat-class-agreement", "NaN"),
        ("--min-beat-match-rate", "inf"),
        ("--max-prob-abs-diff", "NaN"),
        ("--max-prob-abs-diff", "-0.5"),
        ("--max-prob-abs-diff", "inf"),
        ("--max-offset-samples", "-1"),
        ("--max-offset-samples", "infinity"),
        ("--max-offset-samples", "abc"),
    ];
    for (flag, raw) in cases {
        let args = compare_args_with_missing_license(&dir, &[flag, raw]);
        cli().args(&args).assert().code(CLAP_USAGE_EXIT).stderr(
            predicate::str::contains(*flag)
                .and(predicate::str::contains(ini.display().to_string()).not()),
        );
    }
}

#[test]
fn missing_ecl_is_argument_error() {
    let dir = TempDir::new().expect("tempdir");
    cli()
        .args([
            "--license-config",
            missing_license_ini(&dir).to_str().expect("utf8"),
            "compare-accel",
        ])
        .assert()
        .code(CLAP_USAGE_EXIT)
        .stderr(predicate::str::contains("<ECL>"));
}

#[test]
fn boundary_values_pass_parsing_and_reach_license_check() {
    let dir = TempDir::new().expect("tempdir");
    let ini = missing_license_ini(&dir);
    for extra in [
        vec![
            "--prob-stride",
            "1",
            "--tolerance-samples",
            "0",
            "--max-windows",
            "1",
        ],
        vec![
            "--min-rhythm-window-agreement",
            "0",
            "--min-beat-match-rate",
            "1",
            "--min-beat-class-agreement",
            "0.95",
            "--max-prob-abs-diff",
            "0",
            "--max-offset-samples",
            "2.5",
        ],
        vec![
            "--provider",
            "cuda",
            "--batch-size",
            "256",
            "--cuda-tf32",
            "off",
            "--cuda-conv1d-pad-to-nc1d",
            "on",
            "--cuda-graph",
            "1",
        ],
    ] {
        let args = compare_args_with_missing_license(&dir, &extra);
        cli()
            .args(&args)
            .assert()
            .code(APP_FAILURE_EXIT)
            .stderr(predicate::str::contains(ini.display().to_string()));
    }
}

fn write_license_ini(dir: &Path, mock: &LicenseMockServer) -> PathBuf {
    let path = dir.join("license.ini");
    std::fs::write(&path, mock.license_ini_section()).expect("write license.ini");
    path
}

/// 24 h ECL (minimum accepted size) with QRS-like pulses at 250 Hz; the file
/// name declares an 11-minute recording (38 windows).
fn write_synthetic_ecl(dir: &Path) -> PathBuf {
    let n = EXPECTED_24H_SAMPLES_250;
    let mut bytes = Vec::with_capacity(n * 2);
    for i in 0..n {
        let phase = (i % 199) as f32 - 99.0;
        let value = 400.0 * (-(phase * phase) / 4.0).exp() + 20.0 * (i as f32 * 0.004).sin();
        let raw12 = (0x0800 + value.round() as i32).clamp(0, 0x0FFF) as u16;
        let word = ((raw12 & 0x0F00) << 4) | (raw12 & 0x00FF);
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    let path = dir.join("1234567890_20250101_0000_0011.ecl");
    std::fs::write(&path, bytes).expect("write synthetic ECL");
    path
}

fn tiny_dynamic_fixture() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("phase2_tiny_dynamic.onnx")
}

/// Mock license server + synthetic ECL + report dir for one real run.
struct RunEnv {
    _mock: LicenseMockServer,
    dir: TempDir,
    ini: PathBuf,
    ecl: PathBuf,
}

impl RunEnv {
    fn new() -> Self {
        let mock = LicenseMockServer::start();
        let dir = TempDir::new().expect("tempdir");
        let ini = write_license_ini(dir.path(), &mock);
        let ecl = write_synthetic_ecl(dir.path());
        Self {
            _mock: mock,
            dir,
            ini,
            ecl,
        }
    }

    fn report_dir(&self) -> PathBuf {
        self.dir.path().join("report")
    }

    fn compare(&self, model: &Path, extra: &[&str]) -> std::process::Output {
        cli()
            .args(["--license-config", self.ini.to_str().expect("utf8")])
            .args([
                "compare-accel",
                self.ecl.to_str().expect("utf8"),
                "--model",
                model.to_str().expect("utf8"),
                "--report-dir",
                self.report_dir().to_str().expect("utf8"),
            ])
            .args(extra)
            .output()
            .expect("run cli")
    }

    fn report_json(&self) -> serde_json::Value {
        serde_json::from_str(
            &std::fs::read_to_string(self.report_dir().join(REPORT_JSON)).expect("report.json"),
        )
        .expect("valid JSON")
    }
}

#[test]
fn passing_thresholds_print_markdown_summary_and_exit_0() {
    let env = RunEnv::new();
    let out = env.compare(
        &tiny_dynamic_fixture(),
        &[
            "--provider",
            "cpu",
            "--batch-size",
            "4",
            "--min-rhythm-window-agreement",
            "1",
            "--min-beat-match-rate",
            "1",
            "--min-beat-class-agreement",
            "1",
            "--max-prob-abs-diff",
            "0",
            "--max-offset-samples",
            "0",
        ],
    );
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    let stderr = String::from_utf8(out.stderr).expect("utf8 stderr");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout:\n{stdout}\nstderr:\n{stderr}"
    );

    assert!(stdout.starts_with(MARKDOWN_TITLE), "{stdout}");
    let md = std::fs::read_to_string(env.report_dir().join(REPORT_MARKDOWN)).expect("report.md");
    assert_eq!(stdout, md, "stdout must be the Markdown summary");

    let json = env.report_json();
    assert_eq!(json["candidate"]["provider"], "cpu");
    assert_eq!(json["candidate"]["batch_size"], 4);
    assert_eq!(json["baseline"]["batch_size"], 1);
    assert_eq!(json["tolerance_samples"], 40);
    assert_eq!(json["verdict"]["passed"], true);
    let metrics: Vec<&str> = json["verdict"]["checks"]
        .as_array()
        .expect("checks")
        .iter()
        .map(|c| c["metric"].as_str().expect("metric"))
        .collect();
    assert_eq!(metrics.len(), 5, "all five thresholds judged: {metrics:?}");
}

#[test]
fn comparison_options_reach_the_run_without_thresholds() {
    let env = RunEnv::new();
    let out = env.compare(
        &tiny_dynamic_fixture(),
        &[
            "--provider",
            "cpu",
            "--tolerance-samples",
            "7",
            "--prob-stride",
            "2",
            "--max-windows",
            "3",
        ],
    );
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    assert_eq!(
        out.status.code(),
        Some(0),
        "stdout:\n{stdout}\nstderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );
    assert!(stdout.starts_with(MARKDOWN_TITLE), "{stdout}");

    let json = env.report_json();
    assert_eq!(json["tolerance_samples"], 7);
    assert_eq!(json["candidate"]["batch_size"], 16, "default batch size");
    assert!(json["verdict"].is_null(), "no thresholds → no verdict");
    let accuracy = &json["files"][0]["accuracy"];
    assert_eq!(accuracy["windows"], 3);
    assert_eq!(
        accuracy["prob_max_abs_diff"]["sampled_windows"], 2,
        "windows 0, 2"
    );
}

#[test]
fn candidate_load_failure_exits_1_with_reason() {
    let env = RunEnv::new();
    let missing = env.dir.path().join("missing-model.onnx");
    let out = env.compare(&missing, &["--provider", "cpu"]);
    let stderr = String::from_utf8(out.stderr).expect("utf8 stderr");
    assert_eq!(out.status.code(), Some(APP_FAILURE_EXIT), "{stderr}");
    assert!(
        stderr.contains("candidate provider unavailable"),
        "{stderr}"
    );
    assert!(out.stdout.is_empty(), "no summary on error");
    assert!(!env.report_dir().join(REPORT_JSON).exists());
}

/// With a usable CUDA EP, TF32 math against the CPU baseline and a zero
/// probability tolerance: the exit code follows the verdict (2 = failed).
/// Without CUDA the candidate is unavailable (exit 1).
#[test]
fn cuda_candidate_exit_code_follows_verdict_or_unavailability() {
    let env = RunEnv::new();
    let cuda = InferenceOptions {
        provider: ExecutionProviderKind::Cuda,
        batch_size: BatchSize::default(),
        cuda: CudaTuning::default(),
    };
    let cuda_usable = Phase2Model::load_with_options(tiny_dynamic_fixture(), &cuda).is_ok();
    let out = env.compare(
        &tiny_dynamic_fixture(),
        &[
            "--provider",
            "cuda",
            "--cuda-tf32",
            "on",
            "--max-prob-abs-diff",
            "0",
        ],
    );
    let stderr = String::from_utf8_lossy(&out.stderr).into_owned();
    if !cuda_usable {
        assert_eq!(out.status.code(), Some(APP_FAILURE_EXIT), "{stderr}");
        assert!(
            stderr.contains("candidate provider unavailable"),
            "{stderr}"
        );
        return;
    }
    let json = env.report_json();
    let passed = json["verdict"]["passed"].as_bool().expect("verdict");
    let expected = if passed { 0 } else { THRESHOLD_FAILED_EXIT };
    assert_eq!(out.status.code(), Some(expected), "{stderr}");
    let stdout = String::from_utf8(out.stdout).expect("utf8 stdout");
    assert!(
        stdout.starts_with(MARKDOWN_TITLE),
        "summary printed either way"
    );
}
