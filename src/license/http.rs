//! Blocking HTTP adapter for the license server ([`ReqwestLicenseClient`]).
//!
//! Uses [`LicenseConfig`] URLs, paths, timeout, and optional Bearer `api_key`.
//! Non-2xx, `allowed: false`, timeout, and transport failures map to
//! [`LicenseError::StartupFailed`] (check) or [`LicenseError::InferenceDenied`] (meter).
//! Error / Debug output must never include the API key plaintext.

use super::client::LicenseClient;
use super::config::LicenseConfig;
use super::types::{LicenseCheckResult, LicenseError, LicenseMeterResult};
use serde::Serialize;
use std::fmt;

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
        let base = reqwest::Url::parse(&self.config.server_url).map_err(|e| {
            LicenseError::Config(format!("invalid license server_url: {e}"))
        })?;
        let joined = path.trim_start_matches('/');
        base.join(joined).map_err(|e| {
            LicenseError::Config(format!("invalid license endpoint path '{path}': {e}"))
        })
    }

    fn apply_auth(&self, req: reqwest::blocking::RequestBuilder) -> reqwest::blocking::RequestBuilder {
        match &self.config.api_key {
            Some(key) => req.header(
                reqwest::header::AUTHORIZATION,
                format!("Bearer {}", key.expose_secret()),
            ),
            None => req,
        }
    }

    fn post_json<B: Serialize>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<(reqwest::StatusCode, String), String> {
        let url = self.endpoint(path).map_err(|e| e.to_string())?;
        let req = self.http.post(url).json(body);
        let req = self.apply_auth(req);
        let resp = req.send().map_err(|e| sanitize_transport_message(&e))?;
        let status = resp.status();
        let text = resp
            .text()
            .map_err(|e| format!("failed to read response body: {e}"))?;
        Ok((status, text))
    }
}

fn sanitize_transport_message(err: &reqwest::Error) -> String {
    // reqwest errors do not include Authorization headers; keep the summary short
    // and never append request builder state that could hold secrets.
    if err.is_timeout() {
        "request timed out".to_string()
    } else if err.is_connect() {
        format!("connection failed: {err}")
    } else {
        format!("transport error: {err}")
    }
}

fn parse_allowed_response(text: &str) -> Result<(bool, Option<String>), String> {
    #[derive(serde::Deserialize)]
    struct Body {
        allowed: bool,
        #[serde(default)]
        message: Option<String>,
    }
    let parsed: Body =
        serde_json::from_str(text).map_err(|e| format!("invalid JSON response: {e}"))?;
    Ok((parsed.allowed, parsed.message))
}

impl LicenseClient for ReqwestLicenseClient {
    fn check_validity(&self) -> Result<LicenseCheckResult, LicenseError> {
        let fail = |msg: String| LicenseError::StartupFailed(msg);

        let (status, text) = self
            .post_json(&self.config.check_path, &serde_json::json!({}))
            .map_err(fail)?;

        if !status.is_success() {
            return Err(fail(format!(
                "validity check HTTP {status}: {}",
                truncate_for_error(&text)
            )));
        }

        let (allowed, message) = parse_allowed_response(&text).map_err(fail)?;
        if !allowed {
            return Err(fail(
                message.unwrap_or_else(|| "validity check denied".into()),
            ));
        }
        Ok(LicenseCheckResult {
            allowed: true,
            message,
        })
    }

    fn authorize_and_meter(&self) -> Result<LicenseMeterResult, LicenseError> {
        let fail = |msg: String| LicenseError::InferenceDenied(msg);

        #[derive(Serialize)]
        struct MeterBody {
            units: u32,
            kind: &'static str,
        }
        let body = MeterBody {
            units: 1,
            kind: "analyze_job",
        };

        let (status, text) = self
            .post_json(&self.config.meter_path, &body)
            .map_err(fail)?;

        if !status.is_success() {
            return Err(fail(format!(
                "authorize and meter HTTP {status}: {}",
                truncate_for_error(&text)
            )));
        }

        let (allowed, message) = parse_allowed_response(&text).map_err(fail)?;
        if !allowed {
            return Err(fail(
                message.unwrap_or_else(|| "authorize and meter denied".into()),
            ));
        }
        Ok(LicenseMeterResult {
            allowed: true,
            message,
        })
    }
}

fn truncate_for_error(text: &str) -> String {
    const MAX: usize = 200;
    let trimmed = text.trim();
    if trimmed.chars().count() <= MAX {
        trimmed.to_string()
    } else {
        let truncated: String = trimmed.chars().take(MAX).collect();
        format!("{truncated}…")
    }
}

#[cfg(test)]
mod tests {
    use super::ReqwestLicenseClient;
    use crate::license::client::LicenseClient;
    use crate::license::config::{LicenseConfig, SecretString};
    use crate::license::types::LicenseError;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::Duration;

    #[derive(Debug, Clone, Default)]
    struct CapturedRequest {
        method: String,
        path: String,
        authorization: Option<String>,
        body: String,
    }

    struct MockResponse {
        status_line: &'static str,
        body: String,
        /// Extra delay before writing the response (for timeout tests).
        delay: Option<Duration>,
    }

    fn spawn_mock(response: MockResponse) -> (String, Arc<Mutex<Option<CapturedRequest>>>, thread::JoinHandle<()>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind mock");
        let addr = listener.local_addr().expect("local addr");
        let captured = Arc::new(Mutex::new(None));
        let captured_clone = Arc::clone(&captured);

        let handle = thread::spawn(move || {
            // Bound accept so a client that never connects cannot hang the suite.
            listener.set_nonblocking(true).ok();
            let deadline = std::time::Instant::now() + Duration::from_secs(5);
            let (mut stream, _) = loop {
                match listener.accept() {
                    Ok(conn) => break conn,
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        if std::time::Instant::now() >= deadline {
                            return;
                        }
                        thread::sleep(Duration::from_millis(20));
                    }
                    Err(_) => return,
                }
            };
            stream.set_nonblocking(false).ok();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .ok();

            let mut buf = Vec::new();
            let mut tmp = [0u8; 1024];
            loop {
                match stream.read(&mut tmp) {
                    Ok(0) => break,
                    Ok(n) => {
                        buf.extend_from_slice(&tmp[..n]);
                        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                            // If Content-Length present, wait for full body.
                            if let Some(cl) = content_length(&buf) {
                                if let Some(header_end) = find_header_end(&buf) {
                                    let body_len = buf.len() - header_end;
                                    if body_len >= cl {
                                        break;
                                    }
                                }
                            } else {
                                break;
                            }
                        }
                    }
                    Err(_) => break,
                }
            }

            let req = parse_http_request(&buf);
            *captured_clone.lock().unwrap() = Some(req);

            if let Some(delay) = response.delay {
                thread::sleep(delay);
            }

            let body = response.body;
            let resp = format!(
                "{}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                response.status_line,
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        });

        (format!("http://{addr}"), captured, handle)
    }

    fn find_header_end(buf: &[u8]) -> Option<usize> {
        buf.windows(4).position(|w| w == b"\r\n\r\n").map(|i| i + 4)
    }

    fn content_length(buf: &[u8]) -> Option<usize> {
        let s = String::from_utf8_lossy(buf);
        for line in s.lines() {
            let lower = line.to_ascii_lowercase();
            if let Some(rest) = lower.strip_prefix("content-length:") {
                return rest.trim().parse().ok();
            }
        }
        None
    }

    fn parse_http_request(buf: &[u8]) -> CapturedRequest {
        let s = String::from_utf8_lossy(buf);
        let (header_part, body) = match s.split_once("\r\n\r\n") {
            Some((h, b)) => (h, b.to_string()),
            None => (s.as_ref(), String::new()),
        };
        let mut lines = header_part.lines();
        let request_line = lines.next().unwrap_or("");
        let mut parts = request_line.split_whitespace();
        let method = parts.next().unwrap_or("").to_string();
        let path = parts.next().unwrap_or("").to_string();

        let mut authorization = None;
        for line in lines {
            if let Some(rest) = line
                .strip_prefix("Authorization:")
                .or_else(|| line.strip_prefix("authorization:"))
            {
                authorization = Some(rest.trim().to_string());
            }
        }

        CapturedRequest {
            method,
            path,
            authorization,
            body,
        }
    }

    fn config_for(
        base: &str,
        api_key: Option<&str>,
        timeout: Duration,
        check_path: &str,
        meter_path: &str,
    ) -> LicenseConfig {
        LicenseConfig {
            server_url: base.to_string(),
            api_key: api_key.map(SecretString::new),
            timeout,
            check_path: check_path.to_string(),
            meter_path: meter_path.to_string(),
        }
    }

    #[test]
    fn check_validity_success_posts_empty_json_to_check_path() {
        let (base, captured, handle) = spawn_mock(MockResponse {
            status_line: "HTTP/1.1 200 OK",
            body: r#"{"allowed":true,"message":"ok"}"#.into(),
            delay: None,
        });
        let client = ReqwestLicenseClient::new(config_for(
            &base,
            Some("test-secret-key"),
            Duration::from_secs(5),
            "/v1/license/check",
            "/v1/license/meter",
        ))
        .expect("client");

        let result = client.check_validity().expect("check ok");
        assert!(result.allowed);
        assert_eq!(result.message.as_deref(), Some("ok"));

        handle.join().expect("mock thread");
        let req = captured.lock().unwrap().clone().expect("captured");
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/v1/license/check");
        assert_eq!(
            req.authorization.as_deref(),
            Some("Bearer test-secret-key")
        );
        let body: serde_json::Value = serde_json::from_str(&req.body).expect("json body");
        assert_eq!(body, serde_json::json!({}));
    }

    #[test]
    fn authorize_and_meter_success_posts_units_and_kind() {
        let (base, captured, handle) = spawn_mock(MockResponse {
            status_line: "HTTP/1.1 200 OK",
            body: r#"{"allowed":true}"#.into(),
            delay: None,
        });
        let client = ReqwestLicenseClient::new(config_for(
            &base,
            None,
            Duration::from_secs(5),
            "/v1/license/check",
            "/v1/license/meter",
        ))
        .expect("client");

        let result = client.authorize_and_meter().expect("meter ok");
        assert!(result.allowed);

        handle.join().expect("mock thread");
        let req = captured.lock().unwrap().clone().expect("captured");
        assert_eq!(req.method, "POST");
        assert_eq!(req.path, "/v1/license/meter");
        assert!(req.authorization.is_none());
        let body: serde_json::Value = serde_json::from_str(&req.body).expect("json body");
        assert_eq!(body["units"], 1);
        assert_eq!(body["kind"], "analyze_job");
    }

    #[test]
    fn check_deny_maps_to_startup_failed() {
        let (base, _captured, handle) = spawn_mock(MockResponse {
            status_line: "HTTP/1.1 200 OK",
            body: r#"{"allowed":false,"message":"license expired"}"#.into(),
            delay: None,
        });
        let client = ReqwestLicenseClient::new(config_for(
            &base,
            None,
            Duration::from_secs(5),
            "/v1/license/check",
            "/v1/license/meter",
        ))
        .expect("client");

        let err = client.check_validity().expect_err("deny");
        assert!(
            matches!(&err, LicenseError::StartupFailed(msg) if msg.contains("license expired")),
            "{err:?}"
        );
        let _ = handle.join();
    }

    #[test]
    fn meter_deny_maps_to_inference_denied() {
        let (base, _captured, handle) = spawn_mock(MockResponse {
            status_line: "HTTP/1.1 200 OK",
            body: r#"{"allowed":false,"message":"quota exceeded"}"#.into(),
            delay: None,
        });
        let client = ReqwestLicenseClient::new(config_for(
            &base,
            None,
            Duration::from_secs(5),
            "/v1/license/check",
            "/v1/license/meter",
        ))
        .expect("client");

        let err = client.authorize_and_meter().expect_err("deny");
        assert!(
            matches!(&err, LicenseError::InferenceDenied(msg) if msg.contains("quota exceeded")),
            "{err:?}"
        );
        let _ = handle.join();
    }

    #[test]
    fn check_non_2xx_maps_to_startup_failed() {
        let (base, _captured, handle) = spawn_mock(MockResponse {
            status_line: "HTTP/1.1 503 Service Unavailable",
            body: r#"{"error":"down"}"#.into(),
            delay: None,
        });
        let client = ReqwestLicenseClient::new(config_for(
            &base,
            None,
            Duration::from_secs(5),
            "/v1/license/check",
            "/v1/license/meter",
        ))
        .expect("client");

        let err = client.check_validity().expect_err("non-2xx");
        assert!(matches!(err, LicenseError::StartupFailed(_)), "{err:?}");
        assert!(err.to_string().contains("startup failed"));
        let _ = handle.join();
    }

    #[test]
    fn meter_non_2xx_maps_to_inference_denied() {
        let (base, _captured, handle) = spawn_mock(MockResponse {
            status_line: "HTTP/1.1 500 Internal Server Error",
            body: "oops".into(),
            delay: None,
        });
        let client = ReqwestLicenseClient::new(config_for(
            &base,
            None,
            Duration::from_secs(5),
            "/v1/license/check",
            "/v1/license/meter",
        ))
        .expect("client");

        let err = client.authorize_and_meter().expect_err("non-2xx");
        assert!(matches!(err, LicenseError::InferenceDenied(_)), "{err:?}");
        assert!(err.to_string().contains("inference denied"));
        let _ = handle.join();
    }

    #[test]
    fn check_timeout_maps_to_startup_failed() {
        let (base, _captured, handle) = spawn_mock(MockResponse {
            status_line: "HTTP/1.1 200 OK",
            body: r#"{"allowed":true}"#.into(),
            delay: Some(Duration::from_secs(3)),
        });
        let client = ReqwestLicenseClient::new(config_for(
            &base,
            None,
            Duration::from_millis(200),
            "/v1/license/check",
            "/v1/license/meter",
        ))
        .expect("client");

        let err = client.check_validity().expect_err("timeout");
        assert!(
            matches!(&err, LicenseError::StartupFailed(msg) if msg.to_lowercase().contains("timed out") || msg.to_lowercase().contains("timeout")),
            "{err:?}"
        );
        let _ = handle.join();
    }

    #[test]
    fn meter_transport_fail_maps_to_inference_denied() {
        // Nothing listening on this port.
        let client = ReqwestLicenseClient::new(config_for(
            "http://127.0.0.1:1",
            None,
            Duration::from_millis(500),
            "/v1/license/check",
            "/v1/license/meter",
        ))
        .expect("client");

        let err = client.authorize_and_meter().expect_err("transport");
        assert!(matches!(err, LicenseError::InferenceDenied(_)), "{err:?}");
        assert!(err.to_string().contains("inference denied"));
    }

    #[test]
    fn errors_and_debug_never_contain_api_key_plaintext() {
        let secret = "super-secret-api-key-must-not-leak";
        let (base, _captured, handle) = spawn_mock(MockResponse {
            status_line: "HTTP/1.1 401 Unauthorized",
            body: r#"{"allowed":false}"#.into(),
            delay: None,
        });
        let client = ReqwestLicenseClient::new(config_for(
            &base,
            Some(secret),
            Duration::from_secs(5),
            "/v1/license/check",
            "/v1/license/meter",
        ))
        .expect("client");

        let debug = format!("{client:?}");
        assert!(
            !debug.contains(secret),
            "Debug must not leak api_key: {debug}"
        );
        assert!(debug.contains("***") || !debug.to_lowercase().contains("super-secret"));

        let err = client.check_validity().expect_err("401");
        let display = err.to_string();
        let err_dbg = format!("{err:?}");
        assert!(!display.contains(secret), "Display leaked: {display}");
        assert!(!err_dbg.contains(secret), "Error Debug leaked: {err_dbg}");
        let _ = handle.join();
    }

    #[test]
    fn invalid_json_response_is_failure() {
        let (base, _captured, handle) = spawn_mock(MockResponse {
            status_line: "HTTP/1.1 200 OK",
            body: "not-json".into(),
            delay: None,
        });
        let client = ReqwestLicenseClient::new(config_for(
            &base,
            None,
            Duration::from_secs(5),
            "/v1/license/check",
            "/v1/license/meter",
        ))
        .expect("client");

        let err = client.check_validity().expect_err("bad json");
        assert!(matches!(err, LicenseError::StartupFailed(_)), "{err:?}");
        let _ = handle.join();
    }
}
