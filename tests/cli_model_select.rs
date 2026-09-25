//! CLI CliModelSelect integration (model-embedding task 5.2).
//!
//! Covers `--model` missing path → non-zero exit + clear message.
//! With license-client CliStartupIntegration, the process requires a reachable
//! license server before any subcommand — tests spin a local mock HTTP server.
//! Embedded build without `--model` is exercised by `tools/check_cli_embed_select.sh`
//! (requires `HOLTER_EMBEDDED_MODEL_PATH` at build time).

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

fn cmd_with_mock_license(ini: &Path) -> Command {
    let mut cmd = Command::cargo_bin("holter-analysis-assist").expect("cli binary");
    cmd.args(["--license-config", ini.to_str().expect("utf8 path")]);
    cmd
}

#[test]
fn infer_window_missing_model_path_exits_nonzero_with_clear_message() {
    let mock = MockLicenseServer::spawn_allowing();
    let dir = TempDir::new().expect("tempdir");
    let ini = write_license_ini(&dir, mock.base_url());

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
    let mock = MockLicenseServer::spawn_allowing();
    let dir = TempDir::new().expect("tempdir");
    let ini = write_license_ini(&dir, mock.base_url());

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
