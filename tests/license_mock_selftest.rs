//! Self-test for the shared license mock server (`tests/common/license_mock.rs`, task 6.2).
//!
//! Verifies the mock speaks the finalized license server contract: verify 200 /
//! usage 201 success (limited and unlimited), every error code with its status and
//! envelope, raw passthrough, unknown path / method handling, and request recording.

mod common;

use common::license_mock::{
    default_error_message, license_ini_section, status_for_code, LicenseMockServer, MockResponse,
    TEST_LICENSE_KEY,
};
use reqwest::blocking::Client;
use serde_json::Value;

fn client() -> Client {
    Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .expect("build reqwest client")
}

fn post(base: &str, path: &str) -> (u16, String) {
    let resp = client()
        .post(format!("{base}{path}"))
        .header("Authorization", format!("Bearer {TEST_LICENSE_KEY}"))
        .send()
        .expect("send request to mock");
    let status = resp.status().as_u16();
    let body = resp.text().expect("read mock body");
    (status, body)
}

fn json(body: &str) -> Value {
    serde_json::from_str(body).unwrap_or_else(|e| panic!("invalid JSON {body:?}: {e}"))
}

#[test]
fn test_license_key_has_valid_format() {
    let hex = TEST_LICENSE_KEY
        .strip_prefix("lk_")
        .expect("key starts with lk_");
    assert_eq!(hex.len(), 32);
    assert!(hex
        .chars()
        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
}

#[test]
fn license_ini_section_uses_new_contract_keys() {
    let mock = LicenseMockServer::start();
    let section = license_ini_section(mock.base_url());
    assert_eq!(
        section,
        format!(
            "[license]\nserver_url={}\nlicense_key={TEST_LICENSE_KEY}\ntimeout_secs=5\n",
            mock.base_url()
        )
    );
    assert_eq!(mock.license_ini_section(), section);
    assert!(mock.base_url().starts_with("http://127.0.0.1:"));
    assert!(!mock.base_url().ends_with('/'));
}

#[test]
fn default_verify_and_usage_are_unlimited_success() {
    let mock = LicenseMockServer::start();

    let (status, body) = post(mock.base_url(), "/v1/licenses/verify");
    assert_eq!(status, 200, "{body}");
    let v = json(&body);
    assert_eq!(v["ok"], true);
    assert_eq!(v["data"]["valid"], true);
    assert_eq!(v["data"]["monthly_limit"], 0);
    assert_eq!(v["data"]["status"], "active");

    let (status, body) = post(mock.base_url(), "/v1/usage");
    assert_eq!(status, 201, "{body}");
    let v = json(&body);
    assert_eq!(v["ok"], true);
    assert_eq!(v["data"]["allowed"], true);
    assert_eq!(v["data"]["monthly_limit"], 0);
    assert!(v["data"]["remaining"].is_null(), "{body}");
    assert!(v["data"]["used"].is_u64(), "{body}");
    assert_eq!(v["data"]["period"]["start"], "2026-09-30T15:00:00Z");
    assert_eq!(v["data"]["period"]["end"], "2026-10-31T15:00:00Z");
    assert_eq!(v["data"]["period"]["timezone"], "Asia/Tokyo");
}

#[test]
fn limited_success_reports_limit_used_and_remaining() {
    let mock = LicenseMockServer::with_responses(
        MockResponse::limited(100, 7),
        MockResponse::limited(100, 7),
    );

    let (status, body) = post(mock.base_url(), "/v1/licenses/verify");
    assert_eq!(status, 200, "{body}");
    let v = json(&body);
    assert_eq!(v["data"]["valid"], true);
    assert_eq!(v["data"]["monthly_limit"], 100);

    let (status, body) = post(mock.base_url(), "/v1/usage");
    assert_eq!(status, 201, "{body}");
    let v = json(&body);
    assert_eq!(v["data"]["allowed"], true);
    assert_eq!(v["data"]["used"], 7);
    assert_eq!(v["data"]["monthly_limit"], 100);
    assert_eq!(v["data"]["remaining"], 93);
}

#[test]
fn unlimited_usage_success_has_zero_limit_and_null_remaining() {
    let mock = LicenseMockServer::with_responses(
        MockResponse::unlimited(),
        MockResponse::unlimited_used(42),
    );
    let (status, body) = post(mock.base_url(), "/v1/usage");
    assert_eq!(status, 201, "{body}");
    let v = json(&body);
    assert_eq!(v["data"]["used"], 42);
    assert_eq!(v["data"]["monthly_limit"], 0);
    assert!(v["data"]["remaining"].is_null(), "{body}");
}

#[test]
fn status_for_code_matches_server_table() {
    assert_eq!(status_for_code("invalid_request"), Some(400));
    assert_eq!(status_for_code("license_invalid"), Some(401));
    assert_eq!(status_for_code("unauthorized"), Some(401));
    assert_eq!(status_for_code("license_suspended"), Some(403));
    assert_eq!(status_for_code("monthly_limit_reached"), Some(403));
    assert_eq!(status_for_code("license_not_found"), Some(404));
    assert_eq!(status_for_code("rate_limited"), Some(429));
    assert_eq!(status_for_code("temporary_failure"), Some(503));
    assert_eq!(status_for_code("something_new"), None);
}

#[test]
fn every_error_code_returns_status_and_envelope_on_both_endpoints() {
    let cases: [(&str, u16, &str); 6] = [
        ("invalid_request", 400, "The request is malformed."),
        ("license_invalid", 401, "The license key is not valid."),
        ("license_suspended", 403, "The license is suspended."),
        (
            "monthly_limit_reached",
            403,
            "The monthly usage limit has been reached.",
        ),
        (
            "rate_limited",
            429,
            "Too many requests. Please retry later.",
        ),
        (
            "temporary_failure",
            503,
            "The service is temporarily unavailable. Please retry later.",
        ),
    ];
    for (code, expected_status, expected_message) in cases {
        assert_eq!(default_error_message(code), Some(expected_message));
        let mock =
            LicenseMockServer::with_responses(MockResponse::error(code), MockResponse::error(code));
        for path in ["/v1/licenses/verify", "/v1/usage"] {
            let (status, body) = post(mock.base_url(), path);
            assert_eq!(status, expected_status, "{code} {path}: {body}");
            let v = json(&body);
            assert_eq!(v["ok"], false, "{body}");
            assert_eq!(v["error"]["code"], code, "{body}");
            assert_eq!(v["error"]["message"], expected_message, "{body}");
            assert!(v.get("data").is_none(), "{body}");
        }
    }
}

#[test]
fn error_with_explicit_status_and_message_is_passed_through() {
    let mock = LicenseMockServer::with_responses(
        MockResponse::error_with("brand_new_code", 418, "Custom message."),
        MockResponse::error_with("rate_limited", 429, "Slow down."),
    );
    let (status, body) = post(mock.base_url(), "/v1/licenses/verify");
    assert_eq!(status, 418, "{body}");
    let v = json(&body);
    assert_eq!(v["error"]["code"], "brand_new_code");
    assert_eq!(v["error"]["message"], "Custom message.");

    let (status, body) = post(mock.base_url(), "/v1/usage");
    assert_eq!(status, 429, "{body}");
    assert_eq!(json(&body)["error"]["message"], "Slow down.");
}

#[test]
fn raw_response_is_passed_through_verbatim() {
    let html = "<html><body>Bad Gateway</body></html>";
    let mock = LicenseMockServer::with_responses(
        MockResponse::unlimited(),
        MockResponse::raw(502, "text/html; charset=utf-8", html),
    );
    let resp = client()
        .post(format!("{}/v1/usage", mock.base_url()))
        .header("Authorization", format!("Bearer {TEST_LICENSE_KEY}"))
        .send()
        .expect("send");
    assert_eq!(resp.status().as_u16(), 502);
    assert_eq!(
        resp.headers()
            .get("content-type")
            .and_then(|h| h.to_str().ok()),
        Some("text/html; charset=utf-8")
    );
    assert_eq!(resp.text().expect("body"), html);
}

#[test]
fn unknown_path_is_404_and_wrong_method_is_405_invalid_request() {
    let mock = LicenseMockServer::start();

    let (status, body) = post(mock.base_url(), "/v1/license/check");
    assert_eq!(status, 404, "{body}");
    let v = json(&body);
    assert_eq!(v["ok"], false);
    assert_eq!(v["error"]["code"], "invalid_request");
    assert_eq!(v["error"]["message"], "The request is malformed.");

    let resp = client()
        .get(format!("{}/v1/licenses/verify", mock.base_url()))
        .send()
        .expect("send");
    assert_eq!(resp.status().as_u16(), 405);
    assert_eq!(
        json(&resp.text().expect("body"))["error"]["code"],
        "invalid_request"
    );

    assert_eq!(mock.verify_hits(), 0);
    assert_eq!(mock.usage_hits(), 0);
    assert_eq!(mock.requests().len(), 2);
}

#[test]
fn counts_are_tracked_per_endpoint() {
    let mock = LicenseMockServer::start();
    assert_eq!((mock.verify_hits(), mock.usage_hits()), (0, 0));

    post(mock.base_url(), "/v1/licenses/verify");
    post(mock.base_url(), "/v1/usage");
    post(mock.base_url(), "/v1/usage");
    post(mock.base_url(), "/v1/usage?x=1");

    assert_eq!(mock.verify_hits(), 1);
    assert_eq!(mock.usage_hits(), 3);
}

#[test]
fn reused_pooled_client_gets_every_response() {
    let mock = LicenseMockServer::start();
    let client = client();
    for _ in 0..3 {
        for (path, expected) in [("/v1/licenses/verify", 200), ("/v1/usage", 201)] {
            let resp = client
                .post(format!("{}{path}", mock.base_url()))
                .header("Authorization", format!("Bearer {TEST_LICENSE_KEY}"))
                .send()
                .expect("send on reused client");
            assert_eq!(resp.status().as_u16(), expected);
            assert_eq!(json(&resp.text().expect("body"))["ok"], true);
        }
    }
    assert_eq!((mock.verify_hits(), mock.usage_hits()), (3, 3));
}

#[test]
fn records_authorization_body_length_and_target() {
    let mock = LicenseMockServer::start();
    post(mock.base_url(), "/v1/licenses/verify");
    client()
        .post(format!("{}/v1/usage", mock.base_url()))
        .header("Authorization", format!("Bearer {TEST_LICENSE_KEY}"))
        .body("hello")
        .send()
        .expect("send with body");

    let expected_auth = format!("Bearer {TEST_LICENSE_KEY}");
    assert_eq!(
        mock.authorizations(),
        vec![expected_auth.clone(), expected_auth]
    );
    assert_eq!(mock.body_lengths(), vec![0, 5]);
    assert_eq!(
        mock.paths(),
        vec!["/v1/licenses/verify".to_string(), "/v1/usage".to_string()]
    );

    let reqs = mock.requests();
    assert_eq!(reqs[0].method, "POST");
    assert_eq!(reqs[1].body, b"hello".to_vec());
    for req in &reqs {
        assert!(!req.target.contains(TEST_LICENSE_KEY));
        assert!(!String::from_utf8_lossy(&req.body).contains(TEST_LICENSE_KEY));
    }
}

#[test]
fn chunked_request_body_length_is_recorded() {
    use std::io::{Read, Write};
    let mock = LicenseMockServer::start();
    let addr = mock.base_url().trim_start_matches("http://").to_string();
    let mut stream = std::net::TcpStream::connect(addr).expect("connect");
    stream
        .write_all(
            b"POST /v1/usage HTTP/1.1\r\nHost: x\r\nTransfer-Encoding: chunked\r\n\r\n3\r\nabc\r\n4\r\ndefg\r\n0\r\n\r\n",
        )
        .expect("write");
    let mut resp = String::new();
    stream.read_to_string(&mut resp).expect("read");
    assert!(resp.starts_with("HTTP/1.1 201 "), "{resp}");
    assert_eq!(mock.body_lengths(), vec![7]);
    assert!(mock.authorizations().is_empty());
    assert_eq!(mock.requests()[0].authorization, None);
}

#[test]
fn responses_can_be_changed_after_start() {
    let mock = LicenseMockServer::start();
    let (status, _) = post(mock.base_url(), "/v1/usage");
    assert_eq!(status, 201);

    mock.set_usage(MockResponse::error("monthly_limit_reached"));
    mock.set_verify(MockResponse::error("license_suspended"));

    let (status, body) = post(mock.base_url(), "/v1/usage");
    assert_eq!(status, 403);
    assert_eq!(json(&body)["error"]["code"], "monthly_limit_reached");
    let (status, body) = post(mock.base_url(), "/v1/licenses/verify");
    assert_eq!(status, 403);
    assert_eq!(json(&body)["error"]["code"], "license_suspended");
    assert_eq!(mock.usage_hits(), 2);
    assert_eq!(mock.verify_hits(), 1);
}
