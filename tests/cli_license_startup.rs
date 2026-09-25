//! CLI CliStartupIntegration (license-client task 4.2).
//!
//! Startup: load license.ini → install Gate → ensure_startup_licensed before any
//! subcommand. Failures exit non-zero with a startup/config message and must not
//! reach analyze. Uses a local mock HTTP license server (no offline bypass).

use assert_cmd::Command;
use predicates::prelude::*;
use std::io::{Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;
use tempfile::TempDir;

/// Persistent mock: each accepted connection gets the same JSON body.
struct MockLicenseServer {
    base_url: String,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl MockLicenseServer {
    fn spawn(allowed: bool, message: Option<&str>) -> Self {
        let body = match message {
            Some(m) => format!(r#"{{"allowed":{allowed},"message":"{m}"}}"#),
            None => format!(r#"{{"allowed":{allowed}}}"#),
        };
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

    fn base_url(&self) -> &str {
        &self.base_url
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

fn write_license_ini(dir: &TempDir, server_url: &str) -> PathBuf {
    let path = dir.path().join("license.ini");
    std::fs::write(
        &path,
        format!("[license]\nserver_url={server_url}\ntimeout_secs=5\n"),
    )
    .expect("write license.ini");
    path
}

fn cmd_with_license(ini: &Path) -> Command {
    let mut cmd = Command::cargo_bin("holter-analysis-assist").expect("cli binary");
    cmd.args(["--license-config", ini.to_str().expect("utf8 path")]);
    cmd
}

#[test]
fn startup_check_denied_exits_nonzero_with_startup_failed_and_skips_analyze() {
    let mock = MockLicenseServer::spawn(false, Some("license expired"));
    let dir = TempDir::new().expect("tempdir");
    let ini = write_license_ini(&dir, mock.base_url());

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
                .or(predicates::str::contains("license expired")),
        );

    assert!(
        !out_csv.exists(),
        "analyze must not run / must not create output when startup license check fails"
    );
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
    let mock = MockLicenseServer::spawn(true, Some("ok"));
    let dir = TempDir::new().expect("tempdir");
    let ini = write_license_ini(&dir, mock.base_url());
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
}

#[test]
fn env_holter_license_ini_is_honored() {
    // Deny via env-resolved ini: proves HOLTER_LICENSE_INI is used for startup gate
    // (without the flag). Must fail as startup, not as missing ONNX.
    let mock = MockLicenseServer::spawn(false, Some("env deny"));
    let dir = TempDir::new().expect("tempdir");
    let ini = write_license_ini(&dir, mock.base_url());
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
            predicates::str::contains("startup failed").or(predicates::str::contains("env deny")),
        );
}
