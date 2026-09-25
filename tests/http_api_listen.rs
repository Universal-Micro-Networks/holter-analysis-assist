//! http-api listen / analyze integration (tasks 4.1 + 5.2).
//!
//! Covers: startup deny → no listen; startup ok → GET /health 200 (no meter);
//! analyze success / invalid input / inference deny / oversized body; meter
//! exactly once per analyze via the canonical entry (no HTTP-layer double meter).
//!
//! Manual smoke with a real license server and `release-embedded-http-api`
//! artifact is documented in `config/http.ini.example` (task 5.3; not CI-required).
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

struct IniOpts<'a> {
    max_body_bytes: usize,
    model_path: &'a str,
    request_timeout_secs: u64,
}

impl Default for IniOpts<'static> {
    fn default() -> Self {
        Self {
            max_body_bytes: 1_048_576,
            model_path: "/tmp/holter-http-api-listen-missing.onnx",
            request_timeout_secs: 30,
        }
    }
}

fn write_merged_ini(dir: &TempDir, bind: &str, server_url: &str) -> PathBuf {
    write_merged_ini_opts(dir, bind, server_url, IniOpts::default())
}

fn write_merged_ini_opts(
    dir: &TempDir,
    bind: &str,
    server_url: &str,
    opts: IniOpts<'_>,
) -> PathBuf {
    let path = dir.path().join("http-license.ini");
    std::fs::write(
        &path,
        format!(
            "[http]\n\
             bind={bind}\n\
             max_body_bytes={}\n\
             request_timeout_secs={}\n\
             model_path={}\n\
             provider=cpu\n\
             \n\
             [license]\n\
             server_url={server_url}\n\
             timeout_secs=5\n",
            opts.max_body_bytes, opts.request_timeout_secs, opts.model_path
        ),
    )
    .expect("write ini");
    path
}

fn http_exchange(addr: &str, request: &[u8]) -> Result<(u16, Vec<u8>), String> {
    http_exchange_timeout(addr, request, Duration::from_secs(5))
}

fn http_exchange_timeout(
    addr: &str,
    request: &[u8],
    timeout: Duration,
) -> Result<(u16, Vec<u8>), String> {
    let mut stream = TcpStream::connect(addr).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();
    stream
        .write_all(request)
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
        format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n").as_bytes(),
    )
}

fn multipart_analyze_request(ecl_name: &str, ecl_bytes: &[u8]) -> Vec<u8> {
    multipart_analyze_request_with_fields(ecl_name, ecl_bytes, &[])
}

fn multipart_analyze_request_with_fields(
    ecl_name: &str,
    ecl_bytes: &[u8],
    text_fields: &[(&str, &str)],
) -> Vec<u8> {
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
    for (name, value) in text_fields {
        body.extend_from_slice(
            format!(
                "--{boundary}\r\n\
                 Content-Disposition: form-data; name=\"{name}\"\r\n\r\n\
                 {value}\r\n"
            )
            .as_bytes(),
        );
    }
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
    req
}

fn spawn_ready(ini: &Path, bind: &str) -> ChildGuard {
    let mut child = spawn_http_api(ini);
    wait_for_health(bind, Duration::from_secs(10)).unwrap_or_else(|e| {
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
    child
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

    let _child = spawn_ready(&ini, &bind);

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

#[test]
fn analyze_invalid_input_returns_400_without_meter() {
    let mock = MockLicenseServer::spawn(true, true);
    let bind = free_bind_addr();
    let dir = TempDir::new().expect("tempdir");
    let ini = write_merged_ini(&dir, &bind, mock.base_url());
    let _child = spawn_ready(&ini, &bind);

    let before = mock.meter_hits();
    let req = multipart_analyze_request("not-an-ecl.txt", b"not-ecl-bytes");
    let (status, body) = http_exchange(&bind, &req).expect("analyze exchange");

    assert_eq!(
        status, 400,
        "invalid ecl filename must be client error: body={}",
        String::from_utf8_lossy(&body)
    );
    let v: serde_json::Value = serde_json::from_slice(&body).expect("error json");
    assert_eq!(v["error"]["code"], "invalid_input");
    assert_eq!(
        mock.meter_hits().saturating_sub(before),
        0,
        "invalid input must not reach canonical meter"
    );
}

#[test]
fn analyze_meter_deny_returns_403_without_result_leak() {
    let mock = MockLicenseServer::spawn(true, false);
    let bind = free_bind_addr();
    let dir = TempDir::new().expect("tempdir");
    let ini = write_merged_ini(&dir, &bind, mock.base_url());
    let _child = spawn_ready(&ini, &bind);

    let before = mock.meter_hits();
    let req = multipart_analyze_request("1234567890_20240101_0000_2359.ecl", b"placeholder");
    let (status, body) = http_exchange(&bind, &req).expect("analyze exchange");

    assert_eq!(
        status, 403,
        "meter deny must map to inference rejection: body={}",
        String::from_utf8_lossy(&body)
    );
    let v: serde_json::Value = serde_json::from_slice(&body).expect("error json");
    assert_eq!(v["error"]["code"], "license_inference_denied");
    assert!(
        v.get("rows").is_none() && v.get("summary").is_none(),
        "denied response must not leak analyze results: {v}"
    );
    assert_eq!(
        mock.meter_hits().saturating_sub(before),
        1,
        "deny still meters once inside canonical entry; HTTP must not double-meter"
    );
}

#[test]
fn analyze_oversized_body_returns_413_without_meter() {
    let mock = MockLicenseServer::spawn(true, true);
    let bind = free_bind_addr();
    let dir = TempDir::new().expect("tempdir");
    let ini = write_merged_ini_opts(
        &dir,
        &bind,
        mock.base_url(),
        IniOpts {
            max_body_bytes: 64,
            ..IniOpts::default()
        },
    );
    let _child = spawn_ready(&ini, &bind);

    let before = mock.meter_hits();
    // Actual ECL payload exceeds configured max_body_bytes (64).
    // Rejection may come from Content-Length check (JSON envelope) or
    // axum DefaultBodyLimit (413 without body); either is fail-closed.
    let oversized = vec![b'x'; 128];
    let req = multipart_analyze_request("1234567890_20240101_0000_2359.ecl", &oversized);
    let (status, body) = http_exchange(&bind, &req).expect("analyze exchange");

    assert_eq!(
        status, 413,
        "oversized body must be rejected before analyze: body={}",
        String::from_utf8_lossy(&body)
    );
    if !body.is_empty() {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body) {
            assert_eq!(v["error"]["code"], "payload_too_large");
        }
    }
    assert_eq!(
        mock.meter_hits().saturating_sub(before),
        0,
        "oversized body must not meter"
    );
}

#[test]
fn analyze_success_returns_csv_and_meters_once_when_sample_present() {
    let sample_link = Path::new("resources/samples/sample.ecl");
    let onnx = Path::new("resources/models/phase2_rev1.onnx");
    if !sample_link.exists() || !onnx.exists() {
        eprintln!("skip: sample.ecl or ONNX not present");
        return;
    }
    let sample = match sample_link.canonicalize() {
        Ok(p) => p,
        Err(e) => {
            eprintln!("skip: canonicalize sample.ecl: {e}");
            return;
        }
    };
    let ecl_bytes = std::fs::read(&sample).expect("read ecl");
    let ecl_name = sample
        .file_name()
        .and_then(|s| s.to_str())
        .expect("ecl basename");

    let mock = MockLicenseServer::spawn(true, true);
    let bind = free_bind_addr();
    let dir = TempDir::new().expect("tempdir");
    let onnx_abs = onnx.canonicalize().expect("onnx path");
    let ini = write_merged_ini_opts(
        &dir,
        &bind,
        mock.base_url(),
        IniOpts {
            max_body_bytes: 64 * 1024 * 1024,
            model_path: onnx_abs.to_str().expect("onnx utf8"),
            request_timeout_secs: 300,
        },
    );
    let _child = spawn_ready(&ini, &bind);

    let before = mock.meter_hits();
    let req = multipart_analyze_request_with_fields(
        ecl_name,
        &ecl_bytes,
        &[("max_windows", "1"), ("provider", "cpu"), ("format", "csv")],
    );
    let (status, body) =
        http_exchange_timeout(&bind, &req, Duration::from_secs(180)).expect("analyze exchange");
    assert_eq!(
        status, 200,
        "analyze success expected; body={}",
        String::from_utf8_lossy(&body[..body.len().min(500)])
    );
    let csv = String::from_utf8_lossy(&body);
    assert!(
        csv.contains(',') || csv.lines().count() >= 1,
        "CSV body should look like analyze output: {}",
        &csv[..csv.len().min(200)]
    );
    assert_eq!(
        mock.meter_hits().saturating_sub(before),
        1,
        "one successful analyze → one meter"
    );
}
