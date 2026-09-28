//! Blocking HTTP adapter for the license server ([`ReqwestLicenseClient`]).
//!
//! Speaks the finalized client contract relative to [`LicenseConfig`] `server_url`:
//! - `POST v1/licenses/verify` (startup check): success only on HTTP 200 with
//!   `{"ok":true,"data":{"valid":true,..}}`
//! - `POST v1/usage` (per-inference metering): success only on HTTP 201 with
//!   `{"ok":true,"data":{"allowed":true,..}}`
//!
//! Requests carry no body; the license key travels only in
//! `Authorization: Bearer`. Every other outcome (error envelope, non-2xx,
//! unparseable body, transport failure, timeout) is a failure with a
//! [`LicenseFailureReason`]; there is no fail-open path and no automatic retry
//! (a retried usage call could be metered twice).
//!
//! Error / Debug output must never include the license key plaintext.

use super::client::LicenseClient;
use super::config::LicenseConfig;
use super::types::{
    LicenseCheckResult, LicenseError, LicenseFailure, LicenseFailureReason, LicenseMeterResult,
    UsageSnapshot,
};
use reqwest::StatusCode;
use serde::de::DeserializeOwned;
use serde::Deserialize;
use std::fmt;

const VERIFY_PATH: &str = "v1/licenses/verify";
const USAGE_PATH: &str = "v1/usage";
const MAX_SERVER_MESSAGE_CHARS: usize = 200;

/// Wraps a failure in the call-site category (check → startup, meter → inference).
type Wrap = fn(LicenseFailure) -> LicenseError;

/// Blocking reqwest implementation of [`LicenseClient`].
pub struct ReqwestLicenseClient {
    config: LicenseConfig,
    http: reqwest::blocking::Client,
}

impl fmt::Debug for ReqwestLicenseClient {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReqwestLicenseClient")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

#[derive(Deserialize)]
struct Envelope {
    ok: bool,
    #[serde(default)]
    data: Option<serde_json::Value>,
    #[serde(default)]
    error: Option<ErrorBody>,
}

#[derive(Deserialize)]
struct ErrorBody {
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Deserialize)]
struct VerifyData {
    valid: bool,
    #[serde(default)]
    status: Option<String>,
    #[serde(default)]
    monthly_limit: Option<u64>,
}

#[derive(Deserialize)]
struct UsageData {
    allowed: bool,
    used: u64,
    monthly_limit: u64,
    #[serde(default)]
    remaining: Option<u64>,
}

impl ReqwestLicenseClient {
    /// Build a client from validated config (timeout applied to all requests).
    pub fn new(config: LicenseConfig) -> Result<Self, LicenseError> {
        let http = reqwest::blocking::Client::builder()
            .timeout(config.timeout)
            .build()
            .map_err(|e| {
                LicenseError::Config(format!("failed to build license HTTP client: {e}"))
            })?;
        Ok(Self { config, http })
    }

    fn endpoint(&self, path: &str) -> Result<reqwest::Url, LicenseError> {
        let base = reqwest::Url::parse(&self.config.server_url)
            .map_err(|e| LicenseError::Config(format!("invalid license server_url: {e}")))?;
        base.join(path).map_err(|e| {
            LicenseError::Config(format!("invalid license endpoint path '{path}': {e}"))
        })
    }

    /// Bodyless `POST` with the Bearer key; transport failures become
    /// `temporary_failure` in the caller's category.
    fn post(&self, path: &str, wrap: Wrap) -> Result<(StatusCode, String), LicenseError> {
        let url = self.endpoint(path)?;
        let resp = self
            .http
            .post(url)
            .header(reqwest::header::ACCEPT, "application/json")
            .header(reqwest::header::CONTENT_LENGTH, "0")
            .header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", self.config.license_key.expose_secret()),
            )
            .send()
            .map_err(|e| {
                self.failure(
                    wrap,
                    LicenseFailureReason::TemporaryFailure,
                    transport_summary(&e),
                )
            })?;
        let status = resp.status();
        let text = resp.text().map_err(|e| {
            self.failure(
                wrap,
                LicenseFailureReason::TemporaryFailure,
                format!(
                    "failed to read license server response: {}",
                    transport_summary(&e)
                ),
            )
        })?;
        Ok((status, text))
    }

    /// Interpret a response: `data` of the success envelope when the status is
    /// exactly `success_status`, otherwise a classified failure.
    fn interpret<T: DeserializeOwned>(
        &self,
        status: StatusCode,
        text: &str,
        success_status: StatusCode,
        wrap: Wrap,
    ) -> Result<T, LicenseError> {
        let envelope = serde_json::from_str::<Envelope>(text).ok();
        if status.is_success() {
            let unexpected = |detail: String| {
                self.failure(
                    wrap,
                    LicenseFailureReason::UnexpectedResponse,
                    format!(
                        "unexpected license server response (HTTP {}): {detail}",
                        status.as_u16()
                    ),
                )
            };
            if status != success_status {
                return Err(unexpected(format!(
                    "expected HTTP {}",
                    success_status.as_u16()
                )));
            }
            return match envelope {
                Some(Envelope {
                    ok: true,
                    data: Some(data),
                    ..
                }) => serde_json::from_value(data)
                    .map_err(|e| unexpected(format!("invalid data: {e}"))),
                Some(_) => Err(unexpected("not a success envelope".into())),
                None => Err(unexpected("body is not a JSON envelope".into())),
            };
        }

        let error = envelope.filter(|e| !e.ok).and_then(|e| e.error);
        let (code, message) = match error {
            Some(ErrorBody { code, message }) => (code, message),
            None => (None, None),
        };
        let reason = LicenseFailureReason::from_server(code.as_deref(), status.as_u16());
        let message = message
            .map(|m| truncate_chars(&self.redact(m.trim()), MAX_SERVER_MESSAGE_CHARS))
            .filter(|m| !m.is_empty())
            .unwrap_or_else(|| format!("license server returned HTTP {}", status.as_u16()));
        let message = if code.as_deref() == Some("invalid_request")
            && reason == LicenseFailureReason::UnexpectedResponse
        {
            format!(
                "license server returned HTTP {} for this endpoint; check server_url ({message})",
                status.as_u16()
            )
        } else {
            message
        };
        Err(self.failure(wrap, reason, message))
    }

    /// Build a wrapped failure, redacting the key if a message ever echoes it.
    fn failure(&self, wrap: Wrap, reason: LicenseFailureReason, message: String) -> LicenseError {
        wrap(LicenseFailure::new(reason, self.redact(&message)))
    }

    /// Must run before any truncation so a partially cut key cannot slip through.
    fn redact(&self, text: &str) -> String {
        let key = self.config.license_key.expose_secret();
        if key.is_empty() {
            text.to_string()
        } else {
            text.replace(key, "***")
        }
    }
}

fn transport_summary(err: &reqwest::Error) -> String {
    // reqwest errors carry the URL but never request headers; the key is only
    // in the Authorization header, so the summary cannot contain it.
    if err.is_timeout() {
        return "license server request timed out".to_string();
    }
    let mut summary = format!("license server unreachable: {err}");
    let mut source = std::error::Error::source(err);
    while let Some(cause) = source {
        summary.push_str(&format!(": {cause}"));
        source = cause.source();
    }
    summary
}

fn truncate_chars(text: &str, max: usize) -> String {
    text.chars().take(max).collect()
}

impl LicenseClient for ReqwestLicenseClient {
    fn check_validity(&self) -> Result<LicenseCheckResult, LicenseError> {
        let wrap: Wrap = LicenseError::StartupFailed;
        let (status, text) = self.post(VERIFY_PATH, wrap)?;
        let data: VerifyData = self.interpret(status, &text, StatusCode::OK, wrap)?;
        if !data.valid {
            return Err(self.failure(
                wrap,
                LicenseFailureReason::UnexpectedResponse,
                "license server returned valid=false in a success response".into(),
            ));
        }
        Ok(LicenseCheckResult {
            allowed: true,
            status: data.status,
            monthly_limit: data.monthly_limit,
            message: None,
        })
    }

    fn authorize_and_meter(&self) -> Result<LicenseMeterResult, LicenseError> {
        let wrap: Wrap = LicenseError::InferenceDenied;
        let (status, text) = self.post(USAGE_PATH, wrap)?;
        let data: UsageData = self.interpret(status, &text, StatusCode::CREATED, wrap)?;
        if !data.allowed {
            return Err(self.failure(
                wrap,
                LicenseFailureReason::UnexpectedResponse,
                "license server returned allowed=false in a success response".into(),
            ));
        }
        Ok(LicenseMeterResult {
            allowed: true,
            usage: Some(UsageSnapshot {
                used: data.used,
                monthly_limit: data.monthly_limit,
                remaining: data.remaining,
            }),
            message: None,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::ReqwestLicenseClient;
    use crate::license::client::LicenseClient;
    use crate::license::config::{LicenseConfig, SecretString};
    use crate::license::types::{
        LicenseError, LicenseFailure, LicenseFailureReason, UsageSnapshot,
    };
    use serde_json::json;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};

    const TEST_KEY: &str = "lk_0123456789abcdef0123456789abcdef";

    #[derive(Debug, Clone, Default)]
    struct CapturedRequest {
        method: String,
        target: String,
        headers: Vec<(String, String)>,
        body: Vec<u8>,
    }

    impl CapturedRequest {
        fn header(&self, name: &str) -> Option<&str> {
            self.headers
                .iter()
                .find(|(n, _)| n.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        }
    }

    struct MockResponse {
        status: u16,
        body: String,
        /// Delay before writing the response (for timeout tests).
        delay: Option<Duration>,
    }

    impl MockResponse {
        fn new(status: u16, body: impl Into<String>) -> Self {
            Self {
                status,
                body: body.into(),
                delay: None,
            }
        }

        fn delayed(mut self, delay: Duration) -> Self {
            self.delay = Some(delay);
            self
        }
    }

    type Captured = Arc<Mutex<Vec<CapturedRequest>>>;

    /// Serves `response` to every connection. After the first request it keeps
    /// accepting for a short idle window so an (unwanted) retry is recorded too.
    fn spawn_mock(response: MockResponse) -> (String, Captured, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
        let addr = listener.local_addr().expect("local addr");
        listener.set_nonblocking(true).expect("nonblocking");
        let captured: Captured = Arc::new(Mutex::new(Vec::new()));
        let captured_t = Arc::clone(&captured);

        let handle = thread::spawn(move || {
            let first_deadline = Instant::now() + Duration::from_secs(5);
            let mut last_served: Option<Instant> = None;
            loop {
                match listener.accept() {
                    Ok((stream, _)) => {
                        serve_one(stream, &response, &captured_t);
                        last_served = Some(Instant::now());
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        let done = match last_served {
                            Some(t) => t.elapsed() >= Duration::from_millis(300),
                            None => Instant::now() >= first_deadline,
                        };
                        if done {
                            return;
                        }
                        thread::sleep(Duration::from_millis(10));
                    }
                    Err(_) => return,
                }
            }
        });

        (format!("http://{addr}"), captured, handle)
    }

    fn serve_one(mut stream: TcpStream, response: &MockResponse, captured: &Captured) {
        stream.set_nonblocking(false).ok();
        stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
        let mut buf = Vec::new();
        let mut tmp = [0u8; 1024];
        loop {
            match stream.read(&mut tmp) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(header_end) = find_header_end(&buf) {
                        let cl = content_length(&buf[..header_end]).unwrap_or(0);
                        if buf.len() - header_end >= cl {
                            break;
                        }
                    }
                }
            }
        }
        captured.lock().unwrap().push(parse_http_request(&buf));

        if let Some(delay) = response.delay {
            thread::sleep(delay);
        }
        let resp = format!(
            "HTTP/1.1 {} Mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response.status,
            response.body.len(),
            response.body
        );
        let _ = stream.write_all(resp.as_bytes());
        let _ = stream.flush();
    }

    fn find_header_end(buf: &[u8]) -> Option<usize> {
        buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
    }

    fn content_length(head: &[u8]) -> Option<usize> {
        String::from_utf8_lossy(head).lines().find_map(|line| {
            let (name, value) = line.split_once(':')?;
            name.trim()
                .eq_ignore_ascii_case("content-length")
                .then(|| value.trim().parse().ok())?
        })
    }

    fn parse_http_request(buf: &[u8]) -> CapturedRequest {
        let header_end = find_header_end(buf).unwrap_or(buf.len());
        let head = String::from_utf8_lossy(&buf[..header_end]);
        let mut lines = head.split("\r\n");
        let mut request_line = lines.next().unwrap_or("").split_whitespace();
        let method = request_line.next().unwrap_or("").to_string();
        let target = request_line.next().unwrap_or("").to_string();
        let headers = lines
            .filter_map(|line| {
                let (name, value) = line.split_once(':')?;
                Some((name.trim().to_string(), value.trim().to_string()))
            })
            .collect();
        CapturedRequest {
            method,
            target,
            headers,
            body: buf[header_end..].to_vec(),
        }
    }

    /// Mirrors `LicenseConfig::load_from_path` normalization (one trailing `/`).
    fn config_for(base: &str, license_key: &str, timeout: Duration) -> LicenseConfig {
        LicenseConfig {
            server_url: format!("{}/", base.trim_end_matches('/')),
            license_key: SecretString::new(license_key),
            timeout,
        }
    }

    fn client_for(base: &str) -> ReqwestLicenseClient {
        ReqwestLicenseClient::new(config_for(base, TEST_KEY, Duration::from_secs(5)))
            .expect("client")
    }

    fn requests(captured: &Captured, handle: thread::JoinHandle<()>) -> Vec<CapturedRequest> {
        handle.join().expect("mock thread");
        let reqs = captured.lock().unwrap().clone();
        reqs
    }

    fn verify_ok(monthly_limit: u64) -> String {
        json!({
            "ok": true,
            "data": {"valid": true, "status": "active", "monthly_limit": monthly_limit}
        })
        .to_string()
    }

    fn usage_ok(used: u64, monthly_limit: u64, remaining: Option<u64>) -> String {
        json!({
            "ok": true,
            "data": {
                "allowed": true,
                "used": used,
                "monthly_limit": monthly_limit,
                "remaining": remaining,
                "period": {
                    "start": "2026-09-30T15:00:00Z",
                    "end": "2026-10-31T15:00:00Z",
                    "timezone": "Asia/Tokyo"
                },
                "future_field": "ignored"
            }
        })
        .to_string()
    }

    fn error_body(code: &str, message: &str) -> String {
        json!({"ok": false, "error": {"code": code, "message": message}}).to_string()
    }

    fn check_err(response: MockResponse) -> LicenseError {
        let (base, captured, handle) = spawn_mock(response);
        let err = client_for(&base)
            .check_validity()
            .expect_err("check must fail");
        let reqs = requests(&captured, handle);
        assert_eq!(reqs.len(), 1, "no automatic retry: {reqs:?}");
        err
    }

    fn meter_err(response: MockResponse) -> LicenseError {
        let (base, captured, handle) = spawn_mock(response);
        let err = client_for(&base)
            .authorize_and_meter()
            .expect_err("meter must fail");
        let reqs = requests(&captured, handle);
        assert_eq!(reqs.len(), 1, "no automatic retry: {reqs:?}");
        err
    }

    fn startup_failure(err: &LicenseError) -> &LicenseFailure {
        match err {
            LicenseError::StartupFailed(f) => f,
            other => panic!("expected StartupFailed, got {other:?}"),
        }
    }

    fn inference_failure(err: &LicenseError) -> &LicenseFailure {
        match err {
            LicenseError::InferenceDenied(f) => f,
            other => panic!("expected InferenceDenied, got {other:?}"),
        }
    }

    fn closed_port_url() -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
        let addr = listener.local_addr().expect("addr");
        drop(listener);
        format!("http://{addr}")
    }

    // --- success ---

    #[test]
    fn verify_200_valid_is_success_with_status_and_monthly_limit() {
        let (base, captured, handle) = spawn_mock(MockResponse::new(200, verify_ok(100)));
        let result = client_for(&base).check_validity().expect("verify ok");
        assert!(result.allowed);
        assert_eq!(result.status.as_deref(), Some("active"));
        assert_eq!(result.monthly_limit, Some(100));
        assert_eq!(result.message, None);
        assert_eq!(requests(&captured, handle).len(), 1);
    }

    #[test]
    fn verify_unlimited_monthly_limit_zero_is_success() {
        let (base, _captured, handle) = spawn_mock(MockResponse::new(200, verify_ok(0)));
        let result = client_for(&base)
            .check_validity()
            .expect("unlimited verify ok");
        assert!(result.allowed);
        assert_eq!(result.monthly_limit, Some(0));
        handle.join().expect("mock thread");
    }

    #[test]
    fn usage_201_allowed_is_success_with_usage_snapshot() {
        let (base, captured, handle) =
            spawn_mock(MockResponse::new(201, usage_ok(3, 100, Some(97))));
        let result = client_for(&base).authorize_and_meter().expect("usage ok");
        assert!(result.allowed);
        assert_eq!(
            result.usage,
            Some(UsageSnapshot {
                used: 3,
                monthly_limit: 100,
                remaining: Some(97),
            })
        );
        assert_eq!(result.message, None);
        assert_eq!(requests(&captured, handle).len(), 1);
    }

    #[test]
    fn usage_unlimited_with_null_remaining_is_success() {
        let (base, _captured, handle) = spawn_mock(MockResponse::new(201, usage_ok(42, 0, None)));
        let result = client_for(&base)
            .authorize_and_meter()
            .expect("unlimited usage ok");
        assert!(result.allowed);
        let usage = result.usage.expect("usage snapshot");
        assert!(usage.is_unlimited());
        assert_eq!(usage.used, 42);
        assert_eq!(usage.remaining, None);
        handle.join().expect("mock thread");
    }

    // --- 2xx that does not meet the success condition ---

    #[test]
    fn usage_200_is_unexpected_response() {
        let err = meter_err(MockResponse::new(200, usage_ok(1, 10, Some(9))));
        assert_eq!(
            inference_failure(&err).reason,
            LicenseFailureReason::UnexpectedResponse
        );
    }

    #[test]
    fn verify_201_is_unexpected_response() {
        let err = check_err(MockResponse::new(201, verify_ok(10)));
        assert_eq!(
            startup_failure(&err).reason,
            LicenseFailureReason::UnexpectedResponse
        );
    }

    #[test]
    fn verify_valid_false_is_unexpected_response() {
        let body =
            json!({"ok": true, "data": {"valid": false, "status": "active", "monthly_limit": 0}});
        let err = check_err(MockResponse::new(200, body.to_string()));
        assert_eq!(
            startup_failure(&err).reason,
            LicenseFailureReason::UnexpectedResponse
        );
    }

    #[test]
    fn usage_allowed_false_is_unexpected_response() {
        let body = json!({"ok": true, "data": {"allowed": false, "used": 1, "monthly_limit": 1, "remaining": 0}});
        let err = meter_err(MockResponse::new(201, body.to_string()));
        assert_eq!(
            inference_failure(&err).reason,
            LicenseFailureReason::UnexpectedResponse
        );
    }

    #[test]
    fn ok_false_on_2xx_is_unexpected_response() {
        let err = check_err(MockResponse::new(
            200,
            error_body("license_invalid", "The license key is not valid."),
        ));
        assert_eq!(
            startup_failure(&err).reason,
            LicenseFailureReason::UnexpectedResponse
        );
        let err = meter_err(MockResponse::new(
            201,
            error_body("monthly_limit_reached", "limit"),
        ));
        assert_eq!(
            inference_failure(&err).reason,
            LicenseFailureReason::UnexpectedResponse
        );
    }

    #[test]
    fn legacy_contract_body_is_unexpected_response() {
        let err = check_err(MockResponse::new(200, r#"{"allowed":true}"#));
        assert_eq!(
            startup_failure(&err).reason,
            LicenseFailureReason::UnexpectedResponse
        );
    }

    #[test]
    fn non_json_2xx_is_unexpected_response() {
        let err = check_err(MockResponse::new(200, "not-json"));
        assert_eq!(
            startup_failure(&err).reason,
            LicenseFailureReason::UnexpectedResponse
        );
        let err = meter_err(MockResponse::new(201, "<html>ok</html>"));
        assert_eq!(
            inference_failure(&err).reason,
            LicenseFailureReason::UnexpectedResponse
        );
    }

    // --- server error codes ---

    const SERVER_CODES: [(&str, u16, LicenseFailureReason); 6] = [
        ("invalid_request", 400, LicenseFailureReason::InvalidRequest),
        ("license_invalid", 401, LicenseFailureReason::LicenseInvalid),
        (
            "license_suspended",
            403,
            LicenseFailureReason::LicenseSuspended,
        ),
        (
            "monthly_limit_reached",
            403,
            LicenseFailureReason::MonthlyLimitReached,
        ),
        ("rate_limited", 429, LicenseFailureReason::RateLimited),
        (
            "temporary_failure",
            503,
            LicenseFailureReason::TemporaryFailure,
        ),
    ];

    #[test]
    fn server_error_codes_map_to_reasons_as_startup_failed_on_verify() {
        for (code, status, reason) in SERVER_CODES {
            let message = format!("server says {code}");
            let err = check_err(MockResponse::new(status, error_body(code, &message)));
            let failure = startup_failure(&err);
            assert_eq!(failure.reason, reason, "{code} / {status}");
            assert_eq!(failure.message, message, "{code} / {status}");
            assert!(err.to_string().contains(code), "{err}");
        }
    }

    #[test]
    fn server_error_codes_map_to_reasons_as_inference_denied_on_usage() {
        for (code, status, reason) in SERVER_CODES {
            let message = format!("server says {code}");
            let err = meter_err(MockResponse::new(status, error_body(code, &message)));
            let failure = inference_failure(&err);
            assert_eq!(failure.reason, reason, "{code} / {status}");
            assert_eq!(failure.message, message, "{code} / {status}");
        }
    }

    #[test]
    fn routing_error_with_invalid_request_code_points_at_server_url() {
        for status in [404, 405] {
            let body = error_body("invalid_request", "The request is malformed.");
            let err = check_err(MockResponse::new(status, body.clone()));
            let failure = startup_failure(&err);
            assert_eq!(failure.reason, LicenseFailureReason::UnexpectedResponse);
            assert!(failure.message.contains("server_url"), "{failure:?}");
            assert!(failure.message.contains(&status.to_string()), "{failure:?}");

            let err = meter_err(MockResponse::new(status, body));
            let failure = inference_failure(&err);
            assert_eq!(failure.reason, LicenseFailureReason::UnexpectedResponse);
            assert!(failure.message.contains("server_url"), "{failure:?}");
        }
    }

    #[test]
    fn unknown_code_falls_back_to_http_status() {
        let cases = [
            (429, LicenseFailureReason::RateLimited),
            (503, LicenseFailureReason::TemporaryFailure),
            (500, LicenseFailureReason::TemporaryFailure),
            (400, LicenseFailureReason::UnexpectedResponse),
            (404, LicenseFailureReason::UnexpectedResponse),
        ];
        for (status, reason) in cases {
            let err = check_err(MockResponse::new(
                status,
                error_body("brand_new_code", "new server message"),
            ));
            let failure = startup_failure(&err);
            assert_eq!(failure.reason, reason, "check / {status}");
            assert_eq!(failure.message, "new server message");

            let err = meter_err(MockResponse::new(
                status,
                error_body("brand_new_code", "new server message"),
            ));
            assert_eq!(inference_failure(&err).reason, reason, "meter / {status}");
        }
    }

    #[test]
    fn non_json_502_is_temporary_failure_with_status_summary() {
        let err = meter_err(MockResponse::new(502, "<html>Bad Gateway</html>"));
        let failure = inference_failure(&err);
        assert_eq!(failure.reason, LicenseFailureReason::TemporaryFailure);
        assert!(failure.message.contains("HTTP 502"), "{failure:?}");

        let err = check_err(MockResponse::new(502, "<html>Bad Gateway</html>"));
        assert_eq!(
            startup_failure(&err).reason,
            LicenseFailureReason::TemporaryFailure
        );
    }

    #[test]
    fn non_json_4xx_is_unexpected_response() {
        let err = check_err(MockResponse::new(404, "Not Found"));
        let failure = startup_failure(&err);
        assert_eq!(failure.reason, LicenseFailureReason::UnexpectedResponse);
        assert!(failure.message.contains("HTTP 404"), "{failure:?}");
    }

    #[test]
    fn error_without_message_falls_back_to_http_status_summary() {
        let body = json!({"ok": false, "error": {"code": "license_suspended"}});
        let err = check_err(MockResponse::new(403, body.to_string()));
        let failure = startup_failure(&err);
        assert_eq!(failure.reason, LicenseFailureReason::LicenseSuspended);
        assert!(failure.message.contains("HTTP 403"), "{failure:?}");
    }

    #[test]
    fn server_message_is_truncated_to_200_chars() {
        let long = "x".repeat(300);
        let err = meter_err(MockResponse::new(
            403,
            error_body("monthly_limit_reached", &long),
        ));
        let failure = inference_failure(&err);
        assert_eq!(failure.reason, LicenseFailureReason::MonthlyLimitReached);
        assert_eq!(failure.message.chars().count(), 200, "{failure:?}");
        assert!(long.starts_with(&failure.message));
    }

    #[test]
    fn echoed_key_straddling_truncation_boundary_leaves_no_fragment() {
        let message = format!("{}{TEST_KEY} tail", "x".repeat(190));
        let err = meter_err(MockResponse::new(
            403,
            error_body("monthly_limit_reached", &message),
        ));
        let failure = inference_failure(&err);
        assert!(!failure.message.contains("lk_"), "{failure:?}");
        assert!(failure.message.contains("***"), "{failure:?}");
    }

    // --- transport ---

    #[test]
    fn verify_timeout_is_temporary_failure() {
        let (base, captured, handle) =
            spawn_mock(MockResponse::new(200, verify_ok(0)).delayed(Duration::from_secs(3)));
        let client = ReqwestLicenseClient::new(config_for(&base, TEST_KEY, Duration::from_secs(1)))
            .expect("client");
        let err = client.check_validity().expect_err("timeout must fail");
        let failure = startup_failure(&err);
        assert_eq!(failure.reason, LicenseFailureReason::TemporaryFailure);
        assert!(failure.message.contains("timed out"), "{failure:?}");
        assert_eq!(requests(&captured, handle).len(), 1, "no automatic retry");
    }

    #[test]
    fn usage_timeout_is_temporary_failure() {
        let (base, captured, handle) = spawn_mock(
            MockResponse::new(201, usage_ok(1, 0, None)).delayed(Duration::from_secs(3)),
        );
        let client = ReqwestLicenseClient::new(config_for(&base, TEST_KEY, Duration::from_secs(1)))
            .expect("client");
        let err = client.authorize_and_meter().expect_err("timeout must fail");
        let failure = inference_failure(&err);
        assert_eq!(failure.reason, LicenseFailureReason::TemporaryFailure);
        assert!(failure.message.contains("timed out"), "{failure:?}");
        assert_eq!(requests(&captured, handle).len(), 1, "no automatic retry");
    }

    #[test]
    fn unreachable_server_is_temporary_failure() {
        let base = closed_port_url();
        let client = client_for(&base);

        let err = client.check_validity().expect_err("unreachable verify");
        let failure = startup_failure(&err);
        assert_eq!(failure.reason, LicenseFailureReason::TemporaryFailure);
        assert!(failure.message.contains("unreachable"), "{failure:?}");

        let err = client.authorize_and_meter().expect_err("unreachable usage");
        assert_eq!(
            inference_failure(&err).reason,
            LicenseFailureReason::TemporaryFailure
        );
    }

    // --- request shape ---

    fn assert_bodyless_bearer_post(req: &CapturedRequest, expected_path: &str) {
        assert_eq!(req.method, "POST");
        assert_eq!(req.target, expected_path);
        assert_eq!(
            req.header("authorization"),
            Some(format!("Bearer {TEST_KEY}").as_str())
        );
        assert_eq!(req.header("accept"), Some("application/json"));
        assert!(req.body.is_empty(), "body must be empty: {:?}", req.body);
        assert_eq!(req.header("content-length"), Some("0"), "{req:?}");
        assert!(req.header("transfer-encoding").is_none(), "{req:?}");
        assert!(!req.target.contains(TEST_KEY), "key leaked into URL");
        for (name, value) in &req.headers {
            if !name.eq_ignore_ascii_case("authorization") {
                assert!(!value.contains(TEST_KEY), "key leaked into header {name}");
            }
        }
    }

    #[test]
    fn verify_request_is_bodyless_post_with_bearer_header() {
        let (base, captured, handle) = spawn_mock(MockResponse::new(200, verify_ok(0)));
        client_for(&base).check_validity().expect("verify ok");
        let reqs = requests(&captured, handle);
        assert_eq!(reqs.len(), 1);
        assert_bodyless_bearer_post(&reqs[0], "/v1/licenses/verify");
    }

    #[test]
    fn usage_request_is_bodyless_post_with_bearer_header() {
        let (base, captured, handle) = spawn_mock(MockResponse::new(201, usage_ok(1, 0, None)));
        client_for(&base).authorize_and_meter().expect("usage ok");
        let reqs = requests(&captured, handle);
        assert_eq!(reqs.len(), 1);
        assert_bodyless_bearer_post(&reqs[0], "/v1/usage");
    }

    #[test]
    fn server_url_path_prefix_is_kept_when_joining_endpoints() {
        let (base, captured, handle) = spawn_mock(MockResponse::new(201, usage_ok(1, 0, None)));
        let client = client_for(&format!("{base}/prefix"));
        client.authorize_and_meter().expect("usage ok");
        let reqs = requests(&captured, handle);
        assert_eq!(reqs[0].target, "/prefix/v1/usage");

        let (base, captured, handle) = spawn_mock(MockResponse::new(200, verify_ok(0)));
        client_for(&format!("{base}/prefix/"))
            .check_validity()
            .expect("verify ok");
        let reqs = requests(&captured, handle);
        assert_eq!(reqs[0].target, "/prefix/v1/licenses/verify");
    }

    // --- secrets ---

    #[test]
    fn errors_and_debug_never_contain_license_key_plaintext() {
        let client_dbg = format!("{:?}", client_for("http://127.0.0.1:9"));
        assert!(!client_dbg.contains(TEST_KEY), "{client_dbg}");

        let echoing = error_body("license_invalid", &format!("bad key {TEST_KEY}"));
        let errors = vec![
            check_err(MockResponse::new(401, echoing.clone())),
            meter_err(MockResponse::new(401, echoing)),
            check_err(MockResponse::new(500, format!("trace: {TEST_KEY}"))),
            meter_err(MockResponse::new(200, "not-json")),
            client_for(&closed_port_url())
                .check_validity()
                .expect_err("unreachable"),
        ];
        for err in errors {
            let display = err.to_string();
            let debug = format!("{err:?}");
            assert!(!display.contains(TEST_KEY), "Display leaked key: {display}");
            assert!(!debug.contains(TEST_KEY), "Debug leaked key: {debug}");
        }
    }
}
