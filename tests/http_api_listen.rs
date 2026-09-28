//! http-api listen / analyze integration (tasks 4.1 + 5.2).
//!
//! Covers: startup deny → no listen; startup ok → GET /health 200 (no meter);
//! analyze success / invalid input / inference deny / oversized body; meter
//! exactly once per analyze via the canonical entry (no HTTP-layer double meter).
//! Revision (task 9.2): usage denial reasons map to 403 / 429 / 503 with a
//! reason-code prefix and no result; an unlimited license (`remaining: null`)
//! analyzes a multi-window job with a single usage call.
//!
//! Manual smoke with a real license server and `release-embedded-http-api`
//! artifact is documented in `config/http.ini.example` (task 5.3; not CI-required).
//!
//! Uses the shared mock license server (`tests/common/license_mock.rs`, finalized contract).

mod common;

use assert_cmd::cargo::cargo_bin;
use common::license_mock::{
    license_ini_section, LicenseMockServer, MockResponse, TEST_LICENSE_KEY,
};
use holter_analysis_assist::preprocess::EXPECTED_24H_SAMPLES_250;
use predicates::prelude::*;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};
use tempfile::TempDir;

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
    /// Extra `[http]` lines (e.g. `batch_size=4\n`).
    extra_http: &'a str,
}

impl Default for IniOpts<'static> {
    fn default() -> Self {
        Self {
            max_body_bytes: 1_048_576,
            // The server loads the model at startup; use the git-tracked synthetic fixture.
            model_path: concat!(
                env!("CARGO_MANIFEST_DIR"),
                "/tests/fixtures/phase2_tiny_dynamic.onnx"
            ),
            request_timeout_secs: 30,
            extra_http: "",
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
             {}\
             \n\
             {}",
            opts.max_body_bytes,
            opts.request_timeout_secs,
            opts.model_path,
            opts.extra_http,
            license_ini_section(server_url)
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
    http_exchange_with_head(addr, request, timeout).map(|(status, _, body)| (status, body))
}

/// Like [`http_exchange_timeout`], also returning the raw response head (status line + headers).
fn http_exchange_with_head(
    addr: &str,
    request: &[u8],
    timeout: Duration,
) -> Result<(u16, String, Vec<u8>), String> {
    let mut stream = TcpStream::connect(addr).map_err(|e| e.to_string())?;
    stream.set_read_timeout(Some(timeout)).ok();
    stream.set_write_timeout(Some(timeout)).ok();
    stream.write_all(request).map_err(|e| e.to_string())?;
    let mut buf = Vec::new();
    stream.read_to_end(&mut buf).map_err(|e| e.to_string())?;
    let text = String::from_utf8_lossy(&buf);
    let status = text
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .and_then(|s| s.parse::<u16>().ok())
        .ok_or_else(|| format!("no status in response: {text}"))?;
    let (head, body) = if let Some(idx) = text.find("\r\n\r\n") {
        (text[..idx].to_string(), buf[idx + 4..].to_vec())
    } else if let Some(idx) = text.find("\n\n") {
        (text[..idx].to_string(), buf[idx + 2..].to_vec())
    } else {
        (text.to_string(), Vec::new())
    };
    Ok((status, head, body))
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
    let mock = LicenseMockServer::with_responses(
        MockResponse::error("license_invalid"),
        MockResponse::unlimited(),
    );
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
            .and(predicate::str::contains("license_invalid"))
            .eval(&stderr),
        "startup failure must be identifiable: {stderr}"
    );

    // Port must remain closed (no listen after deny).
    assert!(
        TcpStream::connect(&bind).is_err(),
        "must not listen on {bind} when startup license check fails"
    );
    assert!(
        mock.verify_hits() >= 1,
        "startup must have called license check"
    );
    assert_eq!(mock.usage_hits(), 0, "startup path must not meter");
}

#[test]
fn startup_license_ok_listens_and_health_returns_200() {
    let mock = LicenseMockServer::start();
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
    assert!(mock.verify_hits() >= 1);
    assert_eq!(
        mock.usage_hits(),
        0,
        "health must not trigger license meter"
    );
}

#[test]
fn startup_missing_model_exits_nonzero_and_does_not_listen() {
    let mock = LicenseMockServer::start();
    let bind = free_bind_addr();
    let dir = TempDir::new().expect("tempdir");
    let missing = dir.path().join("missing-model.onnx");
    let ini = write_merged_ini_opts(
        &dir,
        &bind,
        mock.base_url(),
        IniOpts {
            model_path: missing.to_str().expect("utf8"),
            ..IniOpts::default()
        },
    );

    let output = assert_cmd::Command::cargo_bin("holter-http-api")
        .expect("holter-http-api binary")
        .args(["--config", ini.to_str().unwrap()])
        .timeout(Duration::from_secs(30))
        .output()
        .expect("run");

    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        !output.status.success(),
        "missing model must exit non-zero; stderr={stderr}"
    );
    assert!(
        stderr.contains("model load or warm-up failed") && stderr.contains("missing-model.onnx"),
        "failure reason must be shown: {stderr}"
    );
    assert!(!stderr.contains("listening on"), "{stderr}");
    assert!(
        TcpStream::connect(&bind).is_err(),
        "must not listen on {bind} when the model fails to load"
    );
}

#[test]
fn startup_logs_model_ready_with_configured_batch_before_listening() {
    let mock = LicenseMockServer::start();
    let bind = free_bind_addr();
    let dir = TempDir::new().expect("tempdir");
    let ini = write_merged_ini_opts(
        &dir,
        &bind,
        mock.base_url(),
        IniOpts {
            extra_http: "batch_size=4\n",
            ..IniOpts::default()
        },
    );

    let mut child = spawn_ready(&ini, &bind);
    let _ = child.0.kill();
    let _ = child.0.wait();
    let mut stderr = String::new();
    child
        .0
        .stderr
        .take()
        .expect("piped stderr")
        .read_to_string(&mut stderr)
        .expect("read stderr");

    let ready = stderr
        .lines()
        .position(|l| {
            l.starts_with("holter-http-api: model ready provider=cpu batch_size=4 ")
                && l.contains("model_batch=dynamic")
                && l.contains("cuda_tuning=not_applied")
                && l.contains("warmup_ms=")
        })
        .unwrap_or_else(|| panic!("ready log missing: {stderr}"));
    let listening = stderr
        .lines()
        .position(|l| l.contains("listening on"))
        .unwrap_or_else(|| panic!("listening log missing: {stderr}"));
    assert!(ready < listening, "ready must precede listen: {stderr}");
}

/// Ini whose limits accept [`synthetic_multi_window_ecl`] uploads.
fn write_synthetic_ecl_ini(dir: &TempDir, bind: &str, server_url: &str) -> PathBuf {
    write_merged_ini_opts(
        dir,
        bind,
        server_url,
        IniOpts {
            max_body_bytes: 64 * 1024 * 1024,
            request_timeout_secs: 300,
            ..IniOpts::default()
        },
    )
}

#[test]
fn analyze_request_meters_once_via_canonical_entry() {
    let mock = LicenseMockServer::start();
    let (ecl_name, ecl_bytes) = synthetic_multi_window_ecl();
    let bind = free_bind_addr();
    let dir = TempDir::new().expect("tempdir");
    let ini = write_synthetic_ecl_ini(&dir, &bind, mock.base_url());

    let _child = spawn_ready(&ini, &bind);

    let before = mock.usage_hits();
    let req = multipart_analyze_request_with_fields(
        ecl_name,
        &ecl_bytes,
        &[("max_windows", "2"), ("format", "json")],
    );
    let (status, body) =
        http_exchange_timeout(&bind, &req, Duration::from_secs(180)).expect("analyze exchange");

    assert_eq!(
        status,
        200,
        "analyze success expected; body={}",
        String::from_utf8_lossy(&body[..body.len().min(500)])
    );
    assert_eq!(
        mock.usage_hits().saturating_sub(before),
        1,
        "exactly one meter call per analyze request (no HTTP-layer double meter)"
    );
}

#[test]
fn analyze_short_ecl_returns_400_without_meter() {
    let mock = LicenseMockServer::start();
    let bind = free_bind_addr();
    let dir = TempDir::new().expect("tempdir");
    let ini = write_merged_ini(&dir, &bind, mock.base_url());
    let _child = spawn_ready(&ini, &bind);

    let before = mock.usage_hits();
    let req = multipart_analyze_request("1234567890_20240101_0000_2359.ecl", b"placeholder");
    let (status, body) = http_exchange(&bind, &req).expect("analyze exchange");

    assert_eq!(
        status,
        400,
        "unreadable ECL content must be client error: body={}",
        String::from_utf8_lossy(&body)
    );
    let v: serde_json::Value = serde_json::from_slice(&body).expect("error json");
    assert_eq!(v["error"]["code"], "invalid_input");
    assert_eq!(
        mock.usage_hits().saturating_sub(before),
        0,
        "ECL that cannot reach inference must not consume usage"
    );
}

#[test]
fn analyze_invalid_input_returns_400_without_meter() {
    let mock = LicenseMockServer::start();
    let bind = free_bind_addr();
    let dir = TempDir::new().expect("tempdir");
    let ini = write_merged_ini(&dir, &bind, mock.base_url());
    let _child = spawn_ready(&ini, &bind);

    let before = mock.usage_hits();
    let req = multipart_analyze_request("not-an-ecl.txt", b"not-ecl-bytes");
    let (status, body) = http_exchange(&bind, &req).expect("analyze exchange");

    assert_eq!(
        status,
        400,
        "invalid ecl filename must be client error: body={}",
        String::from_utf8_lossy(&body)
    );
    let v: serde_json::Value = serde_json::from_slice(&body).expect("error json");
    assert_eq!(v["error"]["code"], "invalid_input");
    assert_eq!(
        mock.usage_hits().saturating_sub(before),
        0,
        "invalid input must not reach canonical meter"
    );
}

#[test]
fn analyze_meter_deny_returns_403_without_result_leak() {
    let mock = LicenseMockServer::with_responses(
        MockResponse::unlimited(),
        MockResponse::error("monthly_limit_reached"),
    );
    let (ecl_name, ecl_bytes) = synthetic_multi_window_ecl();
    let bind = free_bind_addr();
    let dir = TempDir::new().expect("tempdir");
    let ini = write_synthetic_ecl_ini(&dir, &bind, mock.base_url());
    let _child = spawn_ready(&ini, &bind);

    let before = mock.usage_hits();
    let req = multipart_analyze_request_with_fields(ecl_name, &ecl_bytes, &[("max_windows", "2")]);
    let (status, body) =
        http_exchange_timeout(&bind, &req, Duration::from_secs(60)).expect("analyze exchange");

    assert_eq!(
        status,
        403,
        "meter deny must map to inference rejection: body={}",
        String::from_utf8_lossy(&body)
    );
    let v: serde_json::Value = serde_json::from_slice(&body).expect("error json");
    assert_eq!(v["error"]["code"], "license_inference_denied");
    assert!(
        v["error"]["message"]
            .as_str()
            .is_some_and(|m| m.starts_with("monthly_limit_reached")),
        "denial reason must be shown: {v}"
    );
    assert!(
        v.get("rows").is_none() && v.get("summary").is_none(),
        "denied response must not leak analyze results: {v}"
    );
    assert_eq!(
        mock.usage_hits().saturating_sub(before),
        1,
        "deny still meters once inside canonical entry; HTTP must not double-meter"
    );
}

#[test]
fn analyze_oversized_body_returns_413_without_meter() {
    let mock = LicenseMockServer::start();
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

    let before = mock.usage_hits();
    // Actual ECL payload exceeds configured max_body_bytes (64).
    // Rejection may come from Content-Length check (JSON envelope) or
    // axum DefaultBodyLimit (413 without body); either is fail-closed.
    let oversized = vec![b'x'; 128];
    let req = multipart_analyze_request("1234567890_20240101_0000_2359.ecl", &oversized);
    let (status, body) = http_exchange(&bind, &req).expect("analyze exchange");

    assert_eq!(
        status,
        413,
        "oversized body must be rejected before analyze: body={}",
        String::from_utf8_lossy(&body)
    );
    if !body.is_empty() {
        if let Ok(v) = serde_json::from_slice::<serde_json::Value>(&body) {
            assert_eq!(v["error"]["code"], "payload_too_large");
        }
    }
    assert_eq!(
        mock.usage_hits().saturating_sub(before),
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

    let mock = LicenseMockServer::start();
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
            ..IniOpts::default()
        },
    );
    let _child = spawn_ready(&ini, &bind);

    let before = mock.usage_hits();
    let req = multipart_analyze_request_with_fields(
        ecl_name,
        &ecl_bytes,
        &[("max_windows", "1"), ("provider", "cpu"), ("format", "csv")],
    );
    let (status, body) =
        http_exchange_timeout(&bind, &req, Duration::from_secs(180)).expect("analyze exchange");
    assert_eq!(
        status,
        200,
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
        mock.usage_hits().saturating_sub(before),
        1,
        "one successful analyze → one meter"
    );
}

/// 24 h ECL (minimum accepted size) with QRS-like pulses at 250 Hz; the file
/// name declares an 11-minute recording (38 windows).
fn synthetic_multi_window_ecl() -> (&'static str, Vec<u8>) {
    let n = EXPECTED_24H_SAMPLES_250;
    let mut bytes = Vec::with_capacity(n * 2);
    for i in 0..n {
        let phase = (i % 199) as f32 - 99.0;
        let value = 400.0 * (-(phase * phase) / 4.0).exp() + 20.0 * (i as f32 * 0.004).sin();
        let raw12 = (0x0800 + value.round() as i32).clamp(0, 0x0FFF) as u16;
        let word = ((raw12 & 0x0F00) << 4) | (raw12 & 0x00FF);
        bytes.extend_from_slice(&word.to_le_bytes());
    }
    ("1234567890_20250101_0000_0011.ecl", bytes)
}

fn assert_no_license_key(head: &str, body: &[u8]) {
    assert!(
        !head.contains(TEST_LICENSE_KEY),
        "response headers must not contain the license key: {head}"
    );
    assert!(
        !String::from_utf8_lossy(body).contains(TEST_LICENSE_KEY),
        "response body must not contain the license key"
    );
}

#[test]
fn analyze_usage_denials_map_to_retryability_status_with_reason_prefix() {
    let mock = LicenseMockServer::start();
    let (ecl_name, ecl_bytes) = synthetic_multi_window_ecl();
    let bind = free_bind_addr();
    let dir = TempDir::new().expect("tempdir");
    let ini = write_synthetic_ecl_ini(&dir, &bind, mock.base_url());
    let _child = spawn_ready(&ini, &bind);

    for (usage_code, status, error_code) in [
        ("monthly_limit_reached", 403, "license_inference_denied"),
        ("rate_limited", 429, "license_rate_limited"),
        ("temporary_failure", 503, "license_temporarily_unavailable"),
    ] {
        mock.set_usage(MockResponse::error(usage_code));
        let before = mock.usage_hits();
        let req = multipart_analyze_request_with_fields(
            ecl_name,
            &ecl_bytes,
            &[("format", "json"), ("max_windows", "2")],
        );
        let (got, head, body) = http_exchange_with_head(&bind, &req, Duration::from_secs(60))
            .expect("analyze exchange");

        assert_eq!(
            got,
            status,
            "usage {usage_code} must map to HTTP {status}: body={}",
            String::from_utf8_lossy(&body)
        );
        let v: serde_json::Value = serde_json::from_slice(&body).expect("error json");
        let obj = v.as_object().expect("json object");
        assert_eq!(
            obj.keys().collect::<Vec<_>>(),
            vec!["error"],
            "usage {usage_code}: error envelope only, no analyze result: {v}"
        );
        assert_eq!(v["error"]["code"], error_code, "usage {usage_code}: {v}");
        let message = v["error"]["message"].as_str().expect("error.message");
        assert!(
            message.starts_with(&format!("{usage_code}: ")),
            "usage {usage_code}: message must start with the reason code: {message}"
        );
        assert_no_license_key(&head, &body);
        assert_eq!(
            mock.usage_hits().saturating_sub(before),
            1,
            "usage {usage_code}: denied request is metered exactly once"
        );
    }
}

#[test]
fn analyze_unlimited_license_succeeds_and_meters_once_for_multi_window_job() {
    let mock =
        LicenseMockServer::with_responses(MockResponse::unlimited(), MockResponse::unlimited());
    let (ecl_name, ecl_bytes) = synthetic_multi_window_ecl();
    let bind = free_bind_addr();
    let dir = TempDir::new().expect("tempdir");
    let ini = write_synthetic_ecl_ini(&dir, &bind, mock.base_url());
    let _child = spawn_ready(&ini, &bind);

    let before = mock.usage_hits();
    let req = multipart_analyze_request_with_fields(
        ecl_name,
        &ecl_bytes,
        &[("provider", "cpu"), ("format", "json")],
    );
    let (status, head, body) =
        http_exchange_with_head(&bind, &req, Duration::from_secs(180)).expect("analyze exchange");

    assert_eq!(
        status,
        200,
        "unlimited license (remaining: null) must not deny: body={}",
        String::from_utf8_lossy(&body[..body.len().min(500)])
    );
    let v: serde_json::Value = serde_json::from_slice(&body).expect("analyze json");
    assert!(
        v.get("error").is_none(),
        "success must not carry an error: {v}"
    );
    assert!(v["rows"].is_array(), "analyze result rows expected");
    let windows = v["summary"]["windows"].as_u64().expect("summary.windows");
    assert!(
        windows > 1,
        "fixture must exercise multiple windows, got {windows}"
    );
    assert_no_license_key(&head, &body);
    assert_eq!(
        mock.usage_hits().saturating_sub(before),
        1,
        "one analyze request with {windows} windows → exactly one usage call"
    );
}
