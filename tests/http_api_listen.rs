//! http-api task 4.1: listen only after startup license gate; routes + meter once.
//!
//! Reuses the local mock license HTTP server pattern from `cli_license_startup.rs`.

use assert_cmd::cargo::cargo_bin;
use predicates::prelude::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

/// Persistent mock: check vs meter paths return JSON; meter hits are counted.
struct MockLicenseServer {
    base_url: String,
    stop: Arc<std::sync::atomic::AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
    meter_hits: Arc<AtomicUsize>,
    check_hits: Arc<AtomicUsize>,
}

impl MockLicenseServer {
    fn spawn(check_allowed: bool, meter_allowed: bool) -> Self {
        let check_body = if check_allowed {
            r#"{"allowed":true,"message":"ok"}"#.to_string()
        } else {
            r#"{"allowed":false,"message":"license expired"}"#.to_string()
        };
        let meter_body = if meter_allowed {
            r#"{"allowed":true,"message":"metered"}"#.to_string()
        } else {
            r#"{"allowed":false,"message":"quota exceeded"}"#.to_string()
        };

        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock license");
        let addr = listener.local_addr().expect("local addr");
        let stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let stop_thread = Arc::clone(&stop);
        let meter_hits = Arc::new(AtomicUsize::new(0));
        let check_hits = Arc::new(AtomicUsize::new(0));
        let meter_hits_t = Arc::clone(&meter_hits);
        let check_hits_t = Arc::clone(&check_hits);

        let handle = thread::spawn(move || {
            listener
                .set_nonblocking(true)
                .expect("nonblocking mock listener");
            while !stop_thread.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((mut stream, _)) => {
                        stream.set_nonblocking(false).ok();
                        stream.set_read_timeout(Some(Duration::from_secs(2))).ok();
                        let mut buf = [0u8; 8192];
                        let n = stream.read(&mut buf).unwrap_or(0);
                        let req = String::from_utf8_lossy(&buf[..n]);
                        let is_meter = req.contains("/v1/license/meter");
                        let is_check = req.contains("/v1/license/check");
                        if is_meter {
                            meter_hits_t.fetch_add(1, Ordering::SeqCst);
                        }
                        if is_check {
                            check_hits_t.fetch_add(1, Ordering::SeqCst);
                        }
                        let body = if is_meter {
                            meter_body.as_str()
                        } else {
                            check_body.as_str()
                        };
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
            meter_hits,
            check_hits,
        }
    }

    fn base_url(&self) -> &str {
        &self.base_url
    }

    fn meter_hits(&self) -> usize {
        self.meter_hits.load(Ordering::SeqCst)
    }

    fn check_hits(&self) -> usize {
        self.check_hits.load(Ordering::SeqCst)
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

fn free_bind_addr() -> String {
    let listener = TcpListener::bind("127.0.0.1:0").expect("ephemeral bind");
    let port = listener.local_addr().expect("addr").port();
    drop(listener);
    format!("127.0.0.1:{port}")
}

fn write_merged_ini(dir: &TempDir, bind: &str, server_url: &str) -> PathBuf {
    let path = dir.path().join("http-license.ini");
    std::fs::write(
        &path,
        format!(
            "[http]\n\
             bind={bind}\n\
             max_body_bytes=1048576\n\
             request_timeout_secs=30\n\
             model_path=/tmp/holter-http-api-listen-missing.onnx\n\
             provider=cpu\n\
             \n\
             [license]\n\
             server_url={server_url}\n\
             timeout_secs=5\n"
        ),
    )
    .expect("write ini");
    path
}

fn http_exchange(addr: &str, request: &str) -> Result<(u16, Vec<u8>), String> {
    let mut stream = TcpStream::connect(addr).map_err(|e| e.to_string())?;
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .ok();
    stream
        .set_write_timeout(Some(Duration::from_secs(5)))
        .ok();
    stream
        .write_all(request.as_bytes())
        .map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&buf);
    let status = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| format!("no status in response: {text}"))?;
    let body = if let Some(idx) = text.find("\r\n\r\n") {
        buf[idx + 4..].to_vec()
    } else if let Some(idx) = text.find("\n\n") {
        buf[idx + 2..].to_vec()
    } else {
        Vec::new()
    };
    Ok((status, body))
}

fn http_get(addr: &str, path: &str) -> Result<(u16, Vec<u8>), String> {
    http_exchange(
        addr,
        &format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n"),
    )
}

fn multipart_analyze_request(ecl_name: &str, ecl_bytes: &[u8]) -> String {
    let boundary = "----HolterHttpListenBoundary";
    let mut body = Vec::new();
    body.extend_from_slice(
        format!(
            "--{boundary}\r\n\
             Content-Disposition: form-data; name=\"ecl\"; filename=\"{ecl_name}\"\r\n\
             Content-Type: application/octet-stream\r\n\r\n"
        )
        .as_bytes(),
    );
    body.extend_from_slice(ecl_bytes);
    body.extend_from_slice(b"\r\n");
    body.extend_from_slice(format!("--{boundary}--\r\n").as_bytes());

    let mut req = format!(
        "POST /v1/analyze HTTP/1.1\r\n\
         Host: localhost\r\n\
         Content-Type: multipart/form-data; boundary={boundary}\r\n\
         Content-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    req.extend_from_slice(&body);
    String::from_utf8(req).expect("request utf8")
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn spawn_http_api(ini: &Path) -> ChildGuard {
    let bin = cargo_bin("holter-http-api");
    let child = Command::new(bin)
        .args(["--config", ini.to_str().expect("utf8")])
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("spawn holter-http-api");
    ChildGuard(child)
}

fn wait_for_health(addr: &str, timeout: Duration) -> Result<(u16, Vec<u8>), String> {
    let start = Instant::now();
    let mut last = String::new();
    while start.elapsed() < timeout {
        match http_get(addr, "/health") {
            Ok(pair) => return Ok(pair),
            Err(e) => {
                last = e;
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
    Err(format!("health not ready within {timeout:?}: {last}"))
}

#[test]
fn startup_license_denied_exits_nonzero_and_does_not_listen() {
    let mock = MockLicenseServer::spawn(false, true);
    let bind = free_bind_addr();
    let dir = TempDir::new().expect("tempdir");
    let ini = write_merged_ini(&dir, &bind, mock.base_url());

    let output = assert_cmd::Command::cargo_bin("holter-http-api")
        .expect("holter-http-api binary")
        .args(["--config", ini.to_str().unwrap()])
        .output()
        .expect("run");

    assert!(
        !output.status.success(),
        "startup deny must exit non-zero; stderr={}",
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        predicate::str::contains("startup failed")
            .or(predicate::str::contains("license expired"))
            .or(predicate::str::contains("license"))
            .eval(&stderr),
        "startup failure must be identifiable: {stderr}"
    );

    // Port must remain closed (no listen after deny).
    assert!(
        TcpStream::connect(&bind).is_err(),
        "must not listen on {bind} when startup license check fails"
    );
    assert!(
        mock.check_hits() >= 1,
        "startup must have called license check"
    );
    assert_eq!(
        mock.meter_hits(),
        0,
        "startup path must not meter"
    );
}

#[test]
fn startup_license_ok_listens_and_health_returns_200() {
    let mock = MockLicenseServer::spawn(true, true);
    let bind = free_bind_addr();
    let dir = TempDir::new().expect("tempdir");
    let ini = write_merged_ini(&dir, &bind, mock.base_url());

    let mut child = spawn_http_api(&ini);
    let (status, body) = wait_for_health(&bind, Duration::from_secs(10)).unwrap_or_else(|e| {
        let _ = child.0.kill();
        let stderr = child
            .0
            .stderr
            .take()
            .map(|mut s| {
                let mut buf = String::new();
                let _ = s.read_to_string(&mut buf);
                buf
            })
            .unwrap_or_default();
        panic!("health wait failed: {e}; stderr={stderr}");
    });

    assert_eq!(status, 200, "body={}", String::from_utf8_lossy(&body));
    let v: serde_json::Value = serde_json::from_slice(&body).expect("health json");
    assert_eq!(v["status"], "ok");
    assert!(mock.check_hits() >= 1);
    assert_eq!(
        mock.meter_hits(),
        0,
        "health must not trigger license meter"
    );
}

#[test]
fn analyze_request_meters_once_via_canonical_entry() {
    let mock = MockLicenseServer::spawn(true, true);
    let bind = free_bind_addr();
    let dir = TempDir::new().expect("tempdir");
    let ini = write_merged_ini(&dir, &bind, mock.base_url());

    let mut child = spawn_http_api(&ini);
    wait_for_health(&bind, Duration::from_secs(10)).unwrap_or_else(|e| {
        let _ = child.0.kill();
        panic!("health wait failed: {e}");
    });

    let before = mock.meter_hits();
    let req = multipart_analyze_request("1234567890_20240101_0000_2359.ecl", b"placeholder");
    let (status, body) = http_exchange(&bind, &req).expect("analyze exchange");

    // Missing ONNX / placeholder ECL fails after license meter inside canonical entry.
    assert!(
        status == 500 || status == 400 || status == 403,
        "expected post-meter failure status, got {status}; body={}",
        String::from_utf8_lossy(&body)
    );
    assert_eq!(
        mock.meter_hits().saturating_sub(before),
        1,
        "exactly one meter call per analyze request (no HTTP-layer double meter)"
    );
}
