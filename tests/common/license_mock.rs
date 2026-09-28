//! Local mock license server speaking the finalized license server contract.
//!
//! - `POST /v1/licenses/verify` → 200 `{"ok":true,"data":{"valid":true,"monthly_limit":N,"status":"active"}}`
//! - `POST /v1/usage` → 201 `{"ok":true,"data":{"allowed":true,"used":U,"monthly_limit":N,"remaining":R|null,"period":{..}}}`
//! - errors → `{"ok":false,"error":{"code":C,"message":M}}` with the server's status per code
//! - other paths → 404 / wrong method → 405, both `invalid_request` (Flask `HTTPException` handling)
//!
//! Verify and usage responses are scripted independently and can be changed after
//! start. Every request is recorded (method, target, `Authorization`, body) so tests
//! can assert the license key travels only in the `Authorization` header.

use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::thread;
use std::time::Duration;

/// Test license key in the valid format (`lk_` + 32 lowercase hex digits).
pub const TEST_LICENSE_KEY: &str = "lk_0123456789abcdef0123456789abcdef";

pub const VERIFY_PATH: &str = "/v1/licenses/verify";
pub const USAGE_PATH: &str = "/v1/usage";

const PERIOD_START: &str = "2026-09-30T15:00:00Z";
const PERIOD_END: &str = "2026-10-31T15:00:00Z";
const PERIOD_TIMEZONE: &str = "Asia/Tokyo";

const MAX_HEAD_BYTES: usize = 64 * 1024;

/// `[license]` ini fragment for the finalized contract (`license_key`).
pub fn license_ini_section(server_url: &str) -> String {
    format!("[license]\nserver_url={server_url}\nlicense_key={TEST_LICENSE_KEY}\ntimeout_secs=5\n")
}

/// HTTP status the license server returns for an error code.
pub fn status_for_code(code: &str) -> Option<u16> {
    match code {
        "invalid_request" => Some(400),
        "license_invalid" | "unauthorized" => Some(401),
        "license_suspended" | "monthly_limit_reached" => Some(403),
        "license_not_found" => Some(404),
        "rate_limited" => Some(429),
        "temporary_failure" => Some(503),
        _ => None,
    }
}

/// Default message the license server returns for an error code.
pub fn default_error_message(code: &str) -> Option<&'static str> {
    match code {
        "invalid_request" => Some("The request is malformed."),
        "license_invalid" => Some("The license key is not valid."),
        "license_suspended" => Some("The license is suspended."),
        "monthly_limit_reached" => Some("The monthly usage limit has been reached."),
        "unauthorized" => Some("Authentication is required."),
        "license_not_found" => Some("The license does not exist."),
        "rate_limited" => Some("Too many requests. Please retry later."),
        "temporary_failure" => Some("The service is temporarily unavailable. Please retry later."),
        _ => None,
    }
}

/// Scripted reply for one endpoint.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum MockResponse {
    /// Verify → 200 `valid:true`; usage → 201 `allowed:true`. `monthly_limit == 0` is unlimited.
    Success {
        monthly_limit: u64,
        used: u64,
        remaining: Option<u64>,
    },
    /// Error envelope with the given code, status and message.
    Error {
        code: String,
        status: u16,
        message: String,
    },
    /// Arbitrary status and body, sent verbatim.
    Raw {
        status: u16,
        content_type: String,
        body: String,
    },
}

impl MockResponse {
    /// Limited success; `remaining` is `monthly_limit - used` (saturating).
    pub fn limited(monthly_limit: u64, used: u64) -> Self {
        Self::Success {
            monthly_limit,
            used,
            remaining: Some(monthly_limit.saturating_sub(used)),
        }
    }

    /// Unlimited success (`monthly_limit: 0`, `remaining: null`, `used: 0`).
    pub fn unlimited() -> Self {
        Self::unlimited_used(0)
    }

    /// Unlimited success reporting the given `used` count.
    pub fn unlimited_used(used: u64) -> Self {
        Self::Success {
            monthly_limit: 0,
            used,
            remaining: None,
        }
    }

    /// Server error with the server's status and default message for `code`.
    ///
    /// Panics for codes the server does not define; use [`MockResponse::error_with`] instead.
    pub fn error(code: &str) -> Self {
        let status = status_for_code(code)
            .unwrap_or_else(|| panic!("unknown license error code {code:?}; use error_with"));
        let message = default_error_message(code).unwrap_or_default();
        Self::error_with(code, status, message)
    }

    /// Error envelope with an explicit code, status and message (including unknown codes).
    pub fn error_with(code: &str, status: u16, message: &str) -> Self {
        Self::Error {
            code: code.to_string(),
            status,
            message: message.to_string(),
        }
    }

    /// Raw response, e.g. a 502 HTML page from a proxy.
    pub fn raw(status: u16, content_type: &str, body: &str) -> Self {
        Self::Raw {
            status,
            content_type: content_type.to_string(),
            body: body.to_string(),
        }
    }
}

/// One request received by the mock.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct RecordedRequest {
    pub method: String,
    /// Request target as sent (path plus any query string).
    pub target: String,
    pub authorization: Option<String>,
    pub body: Vec<u8>,
}

impl RecordedRequest {
    /// Path without the query string.
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or("")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Endpoint {
    Verify,
    Usage,
}

struct State {
    verify: MockResponse,
    usage: MockResponse,
    verify_hits: usize,
    usage_hits: usize,
    requests: Vec<RecordedRequest>,
}

fn lock(state: &Mutex<State>) -> MutexGuard<'_, State> {
    state.lock().unwrap_or_else(|e| e.into_inner())
}

/// Background-thread HTTP/1.1 server on `127.0.0.1:0`; stops on drop.
pub struct LicenseMockServer {
    base_url: String,
    state: Arc<Mutex<State>>,
    stop: Arc<AtomicBool>,
    handle: Option<thread::JoinHandle<()>>,
}

impl LicenseMockServer {
    /// Verify and usage both answer unlimited success.
    pub fn start() -> Self {
        Self::with_responses(MockResponse::unlimited(), MockResponse::unlimited())
    }

    pub fn with_responses(verify: MockResponse, usage: MockResponse) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind license mock");
        let addr = listener.local_addr().expect("license mock local addr");
        listener
            .set_nonblocking(true)
            .expect("nonblocking license mock listener");
        let state = Arc::new(Mutex::new(State {
            verify,
            usage,
            verify_hits: 0,
            usage_hits: 0,
            requests: Vec::new(),
        }));
        let stop = Arc::new(AtomicBool::new(false));
        let state_t = Arc::clone(&state);
        let stop_t = Arc::clone(&stop);
        let handle = thread::spawn(move || {
            while !stop_t.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => {
                        let state_c = Arc::clone(&state_t);
                        thread::spawn(move || handle_connection(stream, &state_c));
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(5));
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            base_url: format!("http://{addr}"),
            state,
            stop,
            handle: Some(handle),
        }
    }

    /// `http://127.0.0.1:<port>` (no trailing slash), for `server_url`.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    /// `[license]` ini fragment pointing at this server.
    pub fn license_ini_section(&self) -> String {
        license_ini_section(&self.base_url)
    }

    pub fn set_verify(&self, response: MockResponse) {
        lock(&self.state).verify = response;
    }

    pub fn set_usage(&self, response: MockResponse) {
        lock(&self.state).usage = response;
    }

    /// `POST /v1/licenses/verify` calls received.
    pub fn verify_hits(&self) -> usize {
        lock(&self.state).verify_hits
    }

    /// `POST /v1/usage` calls received.
    pub fn usage_hits(&self) -> usize {
        lock(&self.state).usage_hits
    }

    /// All requests in arrival order, including unknown paths.
    pub fn requests(&self) -> Vec<RecordedRequest> {
        lock(&self.state).requests.clone()
    }

    /// `Authorization` header values of requests that carried one.
    pub fn authorizations(&self) -> Vec<String> {
        lock(&self.state)
            .requests
            .iter()
            .filter_map(|r| r.authorization.clone())
            .collect()
    }

    pub fn body_lengths(&self) -> Vec<usize> {
        lock(&self.state)
            .requests
            .iter()
            .map(|r| r.body.len())
            .collect()
    }

    /// Request targets as sent (path plus any query string).
    pub fn paths(&self) -> Vec<String> {
        lock(&self.state)
            .requests
            .iter()
            .map(|r| r.target.clone())
            .collect()
    }
}

impl Drop for LicenseMockServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(h) = self.handle.take() {
            let _ = h.join();
        }
    }
}

fn handle_connection(mut stream: TcpStream, state: &Mutex<State>) {
    stream.set_nonblocking(false).ok();
    stream.set_read_timeout(Some(Duration::from_secs(5))).ok();
    let Some(request) = read_request(&mut stream) else {
        return;
    };

    let endpoint = match (request.method.as_str(), request.path()) {
        ("POST", VERIFY_PATH) => Some(Endpoint::Verify),
        ("POST", USAGE_PATH) => Some(Endpoint::Usage),
        _ => None,
    };
    let known_path = matches!(request.path(), VERIFY_PATH | USAGE_PATH);

    let response = {
        let mut st = lock(state);
        let scripted = match endpoint {
            Some(Endpoint::Verify) => {
                st.verify_hits += 1;
                Some(st.verify.clone())
            }
            Some(Endpoint::Usage) => {
                st.usage_hits += 1;
                Some(st.usage.clone())
            }
            None => None,
        };
        st.requests.push(request);
        scripted
    };

    let (status, content_type, body) = match (endpoint, response) {
        (Some(ep), Some(resp)) => render(ep, &resp),
        _ => {
            let status = if known_path { 405 } else { 404 };
            error_body(status, "invalid_request", "The request is malformed.")
        }
    };
    let head = format!(
        "HTTP/1.1 {status} {}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        reason_phrase(status),
        body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(body.as_bytes());
    let _ = stream.flush();
    let _ = stream.shutdown(std::net::Shutdown::Write);
}

fn render(endpoint: Endpoint, response: &MockResponse) -> (u16, String, String) {
    match response {
        MockResponse::Success {
            monthly_limit,
            used,
            remaining,
        } => {
            let (status, data) = match endpoint {
                Endpoint::Verify => (
                    200,
                    json!({"valid": true, "monthly_limit": monthly_limit, "status": "active"}),
                ),
                Endpoint::Usage => (
                    201,
                    json!({
                        "allowed": true,
                        "used": used,
                        "monthly_limit": monthly_limit,
                        "remaining": remaining,
                        "period": {
                            "start": PERIOD_START,
                            "end": PERIOD_END,
                            "timezone": PERIOD_TIMEZONE,
                        },
                    }),
                ),
            };
            (status, "application/json".to_string(), envelope_ok(data))
        }
        MockResponse::Error {
            code,
            status,
            message,
        } => error_body(*status, code, message),
        MockResponse::Raw {
            status,
            content_type,
            body,
        } => (*status, content_type.clone(), body.clone()),
    }
}

fn envelope_ok(data: Value) -> String {
    json!({"ok": true, "data": data}).to_string()
}

fn error_body(status: u16, code: &str, message: &str) -> (u16, String, String) {
    let body = json!({"ok": false, "error": {"code": code, "message": message}}).to_string();
    (status, "application/json".to_string(), body)
}

fn reason_phrase(status: u16) -> &'static str {
    match status {
        200 => "OK",
        201 => "Created",
        400 => "Bad Request",
        401 => "Unauthorized",
        403 => "Forbidden",
        404 => "Not Found",
        405 => "Method Not Allowed",
        429 => "Too Many Requests",
        500 => "Internal Server Error",
        502 => "Bad Gateway",
        503 => "Service Unavailable",
        504 => "Gateway Timeout",
        _ => "Unknown",
    }
}

/// Reads one HTTP/1.1 request (headers plus `Content-Length` or chunked body).
fn read_request(stream: &mut TcpStream) -> Option<RecordedRequest> {
    let mut buf = Vec::new();
    let head_end = loop {
        if let Some(pos) = find(&buf, b"\r\n\r\n") {
            break pos;
        }
        if buf.len() > MAX_HEAD_BYTES || !read_more(stream, &mut buf) {
            return None;
        }
    };
    let head = String::from_utf8_lossy(&buf[..head_end]).into_owned();
    let mut lines = head.split("\r\n");
    let mut request_line = lines.next()?.split_whitespace();
    let method = request_line.next()?.to_string();
    let target = request_line.next()?.to_string();

    let mut authorization = None;
    let mut content_length = 0usize;
    let mut chunked = false;
    for line in lines {
        let Some((name, value)) = line.split_once(':') else {
            continue;
        };
        let value = value.trim();
        match name.trim().to_ascii_lowercase().as_str() {
            "authorization" => authorization = Some(value.to_string()),
            "content-length" => content_length = value.parse().ok()?,
            "transfer-encoding" => chunked = value.to_ascii_lowercase().contains("chunked"),
            _ => {}
        }
    }

    let mut rest = buf.split_off(head_end + 4);
    let body = if chunked {
        loop {
            if let Some(body) = decode_chunked(&rest) {
                break body;
            }
            if !read_more(stream, &mut rest) {
                return None;
            }
        }
    } else {
        while rest.len() < content_length {
            if !read_more(stream, &mut rest) {
                return None;
            }
        }
        rest.truncate(content_length);
        rest
    };

    Some(RecordedRequest {
        method,
        target,
        authorization,
        body,
    })
}

fn read_more(stream: &mut TcpStream, buf: &mut Vec<u8>) -> bool {
    let mut chunk = [0u8; 8192];
    match stream.read(&mut chunk) {
        Ok(0) | Err(_) => false,
        Ok(n) => {
            buf.extend_from_slice(&chunk[..n]);
            true
        }
    }
}

fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    haystack.windows(needle.len()).position(|w| w == needle)
}

/// Decodes a complete chunked body; `None` while more bytes are needed.
fn decode_chunked(data: &[u8]) -> Option<Vec<u8>> {
    let mut body = Vec::new();
    let mut pos = 0;
    loop {
        let line_end = pos + find(&data[pos..], b"\r\n")?;
        let size_line = std::str::from_utf8(&data[pos..line_end]).ok()?;
        let size_hex = size_line.split(';').next().unwrap_or("").trim();
        let size = usize::from_str_radix(size_hex, 16).ok()?;
        pos = line_end + 2;
        if size == 0 {
            let trailer = &data[pos..];
            if trailer.starts_with(b"\r\n") || find(trailer, b"\r\n\r\n").is_some() {
                return Some(body);
            }
            return None;
        }
        if data.len() < pos + size + 2 {
            return None;
        }
        body.extend_from_slice(&data[pos..pos + size]);
        pos += size + 2;
    }
}
