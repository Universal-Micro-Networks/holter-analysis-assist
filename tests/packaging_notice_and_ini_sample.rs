//! packaging-distribution task 1.1: NoticeBundle + PackagingIniSample contract.
//!
//! - packaging/NOTICE must exist with ORT / third-party attribution
//! - PackagingIniSample must assemble a staging sample that includes [http] and
//!   [license] copied/merged from upstream examples (no key redefinition)

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_utf8(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

/// Extract a named ini section body (until the next line-starting `[section]` or EOF).
fn ini_section(text: &str, name: &str) -> String {
    let header = format!("[{name}]");
    let mut found = false;
    let mut body = String::new();
    for line in text.lines() {
        let t = line.trim();
        if !found {
            if t == header {
                found = true;
            }
            continue;
        }
        if t.starts_with('[') && t.ends_with(']') {
            break;
        }
        body.push_str(line);
        body.push('\n');
    }
    assert!(found, "missing section {header} in:\n{text}");
    body
}

fn required_assignment(section: &str, key: &str) -> String {
    for line in section.lines() {
        let t = line.trim();
        if t.is_empty() || t.starts_with('#') || t.starts_with(';') {
            continue;
        }
        if let Some((k, v)) = t.split_once('=') {
            if k.trim() == key {
                return v.trim().to_string();
            }
        }
    }
    panic!("missing required key `{key}` in section:\n{section}");
}

#[test]
fn packaging_notice_exists_with_ort_attribution() {
    let notice_path = repo_root().join("packaging/NOTICE");
    assert!(
        notice_path.is_file(),
        "NoticeBundle positive: packaging/NOTICE must exist at {}",
        notice_path.display()
    );
    let body = read_utf8(&notice_path);
    assert!(
        !body.trim().is_empty(),
        "packaging/NOTICE must be non-empty"
    );
    let lower = body.to_ascii_lowercase();
    assert!(
        lower.contains("onnx runtime") || lower.contains("ort"),
        "packaging/NOTICE must attribute ONNX Runtime / ORT; got:\n{body}"
    );
    assert!(
        lower.contains("third") || lower.contains("notice") || lower.contains("license"),
        "packaging/NOTICE must look like a third-party notice; got:\n{body}"
    );
}

#[test]
fn packaging_ini_sample_includes_http_and_license_from_upstream() {
    let root = repo_root();
    let license_src = root.join("config/license.ini.example");
    let http_src = root.join("config/http.ini.example");
    assert!(
        license_src.is_file(),
        "upstream license sample missing: {}",
        license_src.display()
    );
    assert!(
        http_src.is_file(),
        "upstream http sample missing: {}",
        http_src.display()
    );

    let assemble = root.join("packaging/scripts/assemble-ini-sample.sh");
    assert!(
        assemble.is_file(),
        "PackagingIniSample script missing: {}",
        assemble.display()
    );

    let status = Command::new("bash")
        .arg(&assemble)
        .current_dir(&root)
        .status()
        .unwrap_or_else(|e| panic!("run {}: {e}", assemble.display()));
    assert!(
        status.success(),
        "assemble-ini-sample.sh must exit 0 (got {status})"
    );

    // Staging sample path produced by PackagingIniSample (merged runtime sample).
    let sample = root.join("packaging/out/staging/sample/http.ini.example");
    assert!(
        sample.is_file(),
        "staging sample ini missing after assemble: {}",
        sample.display()
    );

    let sample_text = read_utf8(&sample);
    let sample_http = ini_section(&sample_text, "http");
    let sample_license = ini_section(&sample_text, "license");

    let upstream_http = ini_section(&read_utf8(&http_src), "http");
    let upstream_license = ini_section(&read_utf8(&license_src), "license");

    let sample_bind = required_assignment(&sample_http, "bind");
    let upstream_bind = required_assignment(&upstream_http, "bind");
    assert_eq!(
        sample_bind, upstream_bind,
        "[http] bind must be copied from config/http.ini.example (no redefinition)"
    );

    let sample_url = required_assignment(&sample_license, "server_url");
    let upstream_url = required_assignment(&upstream_license, "server_url");
    assert_eq!(
        sample_url, upstream_url,
        "[license] server_url must be copied from config/license.ini.example (no redefinition)"
    );

    // Ensure we did not invent a packaging-owned key canonical under packaging/.
    assert!(
        !root.join("packaging/packaging.ini.example").is_file(),
        "must not own packaging.ini.example as a key canonical"
    );
}
