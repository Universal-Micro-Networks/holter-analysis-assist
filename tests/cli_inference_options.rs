//! CLI inference options for `analyze-ecl` (inference-acceleration task 3.3).
//!
//! - `--help` lists `--batch-size`, `--cuda-tf32`, `--cuda-conv1d-pad-to-nc1d`,
//!   `--cuda-graph` (same names as the `[http]` ini keys with `_` → `-`).
//! - Invalid values are clap argument errors (exit 2) raised before the CLI
//!   license check, so no license server is needed for them.
//! - A successful analysis keeps the stdout summary unchanged and writes one
//!   `perf:` line to stderr.

use assert_cmd::Command;
use holter_analysis_assist::inference_options::{
    DEFAULT_BATCH_SIZE, KEY_BATCH_SIZE, KEY_CUDA_CONV1D_PAD, KEY_CUDA_GRAPH, KEY_CUDA_TF32,
    MAX_BATCH_SIZE,
};
use holter_analysis_assist::preprocess::EXPECTED_24H_SAMPLES_250;
use predicates::prelude::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

/// clap usage errors exit with 2; license / analysis failures exit with 1.
const CLAP_USAGE_EXIT: i32 = 2;
const APP_FAILURE_EXIT: i32 = 1;

fn cli() -> Command {
    Command::cargo_bin("holter-analysis-assist").expect("cli binary")
}

fn flag(key: &str) -> String {
    format!("--{}", key.replace('_', "-"))
}

/// License ini path that does not exist: reaching the license step fails with exit 1.
fn missing_license_ini(dir: &TempDir) -> PathBuf {
    dir.path().join("no-such-license.ini")
}

fn analyze_args_with_missing_license(dir: &TempDir, extra: &[&str]) -> Vec<String> {
    let mut args = vec![
        "--license-config".to_string(),
        missing_license_ini(dir).display().to_string(),
        "analyze-ecl".to_string(),
        "1234567890_20250101_0000_0011.ecl".to_string(),
        "--provider".to_string(),
        "cpu".to_string(),
        "--output".to_string(),
        dir.path().join("out.csv").display().to_string(),
    ];
    args.extend(extra.iter().map(|s| s.to_string()));
    args
}

#[test]
fn analyze_ecl_help_lists_inference_options() {
    let out = cli()
        .args(["analyze-ecl", "--help"])
        .assert()
        .success()
        .get_output()
        .stdout
        .clone();
    let help = String::from_utf8(out).expect("utf8 help");
    for key in [
        KEY_BATCH_SIZE,
        KEY_CUDA_TF32,
        KEY_CUDA_CONV1D_PAD,
        KEY_CUDA_GRAPH,
    ] {
        assert!(
            help.contains(&flag(key)),
            "missing {} in:\n{help}",
            flag(key)
        );
    }
    assert!(
        help.contains(&format!("{DEFAULT_BATCH_SIZE}"))
            && help.contains(&format!("{MAX_BATCH_SIZE}")),
        "batch-size help must document default and max:\n{help}"
    );
    assert!(
        help.contains("true|false|on|off|1|0"),
        "switch help must document accepted values:\n{help}"
    );
    assert!(help.contains("--provider"), "{help}");
}

#[test]
fn invalid_batch_size_is_argument_error_before_license_check() {
    let dir = TempDir::new().expect("tempdir");
    let ini = missing_license_ini(&dir);
    for raw in ["0", "257", "abc", "-1", "1.5", ""] {
        let args = analyze_args_with_missing_license(&dir, &["--batch-size", raw]);
        cli().args(&args).assert().code(CLAP_USAGE_EXIT).stderr(
            predicate::str::contains("--batch-size")
                .and(predicate::str::contains(KEY_BATCH_SIZE))
                .and(predicate::str::contains("1..=256"))
                .and(predicate::str::contains(ini.display().to_string()).not()),
        );
    }
}

#[test]
fn invalid_cuda_switch_is_argument_error_before_license_check() {
    let dir = TempDir::new().expect("tempdir");
    let ini = missing_license_ini(&dir);
    for key in [KEY_CUDA_TF32, KEY_CUDA_CONV1D_PAD, KEY_CUDA_GRAPH] {
        for raw in ["yes", "2", "enable", ""] {
            let name = flag(key);
            let args = analyze_args_with_missing_license(&dir, &[&name, raw]);
            cli().args(&args).assert().code(CLAP_USAGE_EXIT).stderr(
                predicate::str::contains(name.as_str())
                    .and(predicate::str::contains(key))
                    .and(predicate::str::contains("true|false|on|off|1|0"))
                    .and(predicate::str::contains(ini.display().to_string()).not()),
            );
        }
    }
}

#[test]
fn valid_inference_options_pass_parsing_and_reach_license_check() {
    let dir = TempDir::new().expect("tempdir");
    let ini = missing_license_ini(&dir);
    for extra in [
        vec!["--batch-size", "1"],
        vec!["--batch-size", "256"],
        vec![
            "--batch-size",
            " 32 ",
            "--cuda-tf32",
            "on",
            "--cuda-conv1d-pad-to-nc1d",
            "FALSE",
            "--cuda-graph",
            "1",
        ],
        vec!["--cuda-tf32", "off", "--cuda-graph", "true"],
    ] {
        let args = analyze_args_with_missing_license(&dir, &extra);
        cli()
            .args(&args)
            .assert()
            .code(APP_FAILURE_EXIT)
            .stderr(predicate::str::contains(ini.display().to_string()));
    }
}

struct MockLicenseServer {
    base_url: String,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl MockLicenseServer {
    fn spawn_allowing() -> Self {
        let body = r#"{"allowed":true}"#;
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock license");
        let addr = listener.local_addr().expect("local addr");
        let stop = Arc::new(AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);

        let handle = thread::spawn(move || {
            listener
                .set_nonblocking(true)
                .expect("nonblocking mock listener");
            while !stop_thread.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).ok();
                        stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
                        let mut buf = [0u8; 4096];
                        let _ = stream.read(&mut buf);
                        let resp = format!(
                            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                            body.len(),
                            body
                        );
                        let _ = stream.write_all(resp.as_bytes());
                        let _ = stream.flush();
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => break,
                }
            }
        });

        Self {
            base_url: format!("http://{addr}"),
            stop,
            handle: Some(handle),
        }
    }
}

impl Drop for MockLicenseServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn write_license_ini(dir: &Path, server_url: &str) -> PathBuf {
    let path = dir.join("license.ini");
    std::fs::write(
        &path,
        format!("[license]\nserver_url={server_url}\ntimeout_secs=5\n"),
    )
    .expect("write license.ini");
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

#[test]
fn successful_analysis_keeps_stdout_and_writes_one_perf_line_to_stderr() {
    let mock = MockLicenseServer::spawn_allowing();
    let dir = TempDir::new().expect("tempdir");
    let ini = write_license_ini(dir.path(), &mock.base_url);
    let ecl = write_synthetic_ecl(dir.path());
    let csv = dir.path().join("beat_results.csv");
    let model = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("phase2_tiny_dynamic.onnx");

    let output = cli()
        .args(["--license-config", ini.to_str().expect("utf8")])
        .args([
            "analyze-ecl",
            ecl.to_str().expect("utf8"),
            "--model",
            model.to_str().expect("utf8"),
            "--output",
            csv.to_str().expect("utf8"),
            "--provider",
            "cpu",
            "--batch-size",
            "4",
            "--cuda-tf32",
            "on",
        ])
        .assert()
        .success()
        .get_output()
        .clone();

    let stdout = String::from_utf8(output.stdout).expect("utf8 stdout");
    let stderr = String::from_utf8(output.stderr).expect("utf8 stderr");

    let keys: Vec<&str> = stdout
        .lines()
        .map(|l| l.split_once(": ").expect("key: value line").0)
        .collect();
    assert_eq!(
        keys,
        ["saved", "beats", "windows", "Unknown=1", "short_run_flag=1"],
        "stdout summary must stay unchanged:\n{stdout}"
    );
    assert!(
        stdout.contains(&format!("saved: {}", csv.display())),
        "{stdout}"
    );
    assert!(stdout.contains("windows: 38"), "{stdout}");
    assert!(
        !stdout.contains("perf:"),
        "perf line leaked to stdout:\n{stdout}"
    );

    let perf_lines: Vec<&str> = stderr.lines().filter(|l| l.starts_with("perf: ")).collect();
    assert_eq!(perf_lines.len(), 1, "exactly one perf line:\n{stderr}");
    let perf = perf_lines[0];
    for token in [
        "provider=cpu",
        "batch_size=4",
        "cuda_tuning=not_applied",
        "windows=38",
        "total_ms=",
    ] {
        assert!(perf.contains(token), "missing {token} in {perf}");
    }

    let body = std::fs::read_to_string(&csv).expect("read csv");
    assert!(!body.contains("perf:"), "perf line leaked into CSV");
    assert!(body.lines().count() > 1, "CSV must contain beat rows");
}
