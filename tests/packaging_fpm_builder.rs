//! packaging-distribution task 2.3: FpmBuilder contract (file content + prepare layout).
//!
//! fpm may be unavailable on CI/dev hosts, so we assert packaging/linux/fpm.sh
//! wiring and a `--prepare-only` install-root layout rather than requiring a
//! full `fpm` gem install for every run.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_utf8(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn run_bash(script: &Path, args: &[&str]) -> std::process::Output {
    Command::new("bash")
        .arg(script)
        .args(args)
        .current_dir(repo_root())
        .output()
        .unwrap_or_else(|e| panic!("run {}: {e}", script.display()))
}

fn write_minimal_linux_staging(root: &Path) {
    fs::create_dir_all(root.join("bin")).unwrap();
    fs::write(root.join("bin/holter-http-api"), b"#!/bin/sh\necho mock\n").unwrap();
    fs::write(root.join("NOTICE"), "ORT third-party notices\n").unwrap();
    fs::write(
        root.join("http.ini.example"),
        "[license]\nserver_url=https://license.example.com\n\n[http]\nbind=0.0.0.0:8080\n",
    )
    .unwrap();
    fs::create_dir_all(root.join("docs")).unwrap();
    fs::write(root.join("docs/EMBEDDED_BINARY.md"), "embedded premise\n").unwrap();
}

#[test]
fn fpm_script_exists_and_builds_both_deb_and_rpm() {
    let script = repo_root().join("packaging/linux/fpm.sh");
    assert!(
        script.is_file(),
        "FpmBuilder: packaging/linux/fpm.sh must exist at {}",
        script.display()
    );
    let body = read_utf8(&script);

    assert!(
        body.contains("set -e") || body.contains("set -euo pipefail"),
        "fpm.sh must use set -e (fail closed); got:\n{body}"
    );

    // Both formats from the same staging input (Req 3.1, 3.2 / design FpmBuilder).
    assert!(
        body.contains("-t deb") || body.contains("\"deb\"") || body.contains("'deb'"),
        "fpm.sh must generate a .deb (-t deb); got:\n{body}"
    );
    assert!(
        body.contains("-t rpm") || body.contains("\"rpm\"") || body.contains("'rpm'"),
        "fpm.sh must generate a .rpm (-t rpm); got:\n{body}"
    );
    assert!(
        body.contains("fpm"),
        "fpm.sh must invoke fpm; got:\n{body}"
    );

    // Package name
    assert!(
        body.contains("holter-http-api"),
        "fpm.sh must use package name holter-http-api; got:\n{body}"
    );
}

#[test]
fn fpm_script_targets_amd64_deb_and_x86_64_rpm() {
    let body = read_utf8(&repo_root().join("packaging/linux/fpm.sh"));

    // Design: deb=amd64, rpm=x86_64
    assert!(
        body.contains("amd64"),
        "fpm.sh must set deb architecture amd64; got:\n{body}"
    );
    assert!(
        body.contains("x86_64"),
        "fpm.sh must set rpm architecture x86_64; got:\n{body}"
    );
}

#[test]
fn fpm_script_places_binary_notice_and_ini_at_design_paths() {
    let body = read_utf8(&repo_root().join("packaging/linux/fpm.sh"));

    // Design install paths after package install (Req 3.3, 6.1, 7.1).
    assert!(
        body.contains("/usr/bin/holter-http-api"),
        "fpm.sh must install binary to /usr/bin/holter-http-api; got:\n{body}"
    );
    assert!(
        body.contains("/usr/share/holter-http-api"),
        "fpm.sh must place share files under /usr/share/holter-http-api/; got:\n{body}"
    );
    assert!(
        body.contains("NOTICE"),
        "fpm.sh must include NOTICE under the share path; got:\n{body}"
    );
    assert!(
        body.contains("http.ini.example") || body.contains("ini.example"),
        "fpm.sh must include sample ini under the share path; got:\n{body}"
    );

    // Active packaging must not pull raw model weights (Req 4.1).
    let active: String = body
        .lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with('#')
        })
        .collect::<Vec<_>>()
        .join("\n");
    let active_lower = active.to_ascii_lowercase();
    assert!(
        !active_lower.contains(".onnx")
            && !active_lower.contains("resources/models")
            && !active_lower.contains("*.ort"),
        "active fpm.sh instructions must not reference raw model files; got:\n{active}"
    );
}

#[test]
fn fpm_prepare_only_builds_install_root_from_staging() {
    let script = repo_root().join("packaging/linux/fpm.sh");
    assert!(
        script.is_file(),
        "missing FpmBuilder script: {}",
        script.display()
    );

    let staging = TempDir::new().unwrap();
    write_minimal_linux_staging(staging.path());

    let out = TempDir::new().unwrap();
    let result = run_bash(
        &script,
        &[
            "--staging",
            staging.path().to_str().unwrap(),
            "--output",
            out.path().to_str().unwrap(),
            "--prepare-only",
        ],
    );
    assert!(
        result.status.success(),
        "fpm.sh --prepare-only must exit 0; stderr:\n{}\nstdout:\n{}",
        String::from_utf8_lossy(&result.stderr),
        String::from_utf8_lossy(&result.stdout)
    );

    // Install-root layout that fpm will package (maps to design paths).
    let root = out.path().join("root");
    let binary = root.join("usr/bin/holter-http-api");
    let notice = root.join("usr/share/holter-http-api/NOTICE");
    let ini = root.join("usr/share/holter-http-api/http.ini.example");

    assert!(
        binary.is_file(),
        "prepare-only must place binary at usr/bin/holter-http-api; missing {}",
        binary.display()
    );
    assert!(
        notice.is_file(),
        "prepare-only must place NOTICE at usr/share/holter-http-api/NOTICE; missing {}",
        notice.display()
    );
    assert!(
        ini.is_file(),
        "prepare-only must place sample ini at usr/share/holter-http-api/http.ini.example; missing {}",
        ini.display()
    );

    let notice_body = read_utf8(&notice);
    assert!(
        notice_body.contains("ORT") || notice_body.contains("third-party") || !notice_body.is_empty(),
        "NOTICE content must be copied into the package root"
    );
    let ini_body = read_utf8(&ini);
    assert!(
        ini_body.contains("[license]") && ini_body.contains("[http]"),
        "sample ini must retain [license] and [http]; got:\n{ini_body}"
    );

    // No raw models in prepared root (Req 4.1).
    let mut onnx_found = false;
    if root.exists() {
        for entry in walkdir_files(&root) {
            let name = entry
                .file_name()
                .map(|n| n.to_string_lossy().to_ascii_lowercase())
                .unwrap_or_default();
            if name.ends_with(".onnx") || name.ends_with(".ort") || name.ends_with(".weights.h5") {
                onnx_found = true;
                break;
            }
        }
    }
    assert!(!onnx_found, "prepared package root must not contain raw model files");
}

#[test]
fn fpm_script_fails_closed_when_staging_incomplete() {
    let script = repo_root().join("packaging/linux/fpm.sh");
    assert!(script.is_file(), "missing {}", script.display());

    let staging = TempDir::new().unwrap();
    // Binary only — missing NOTICE and ini.
    fs::create_dir_all(staging.path().join("bin")).unwrap();
    fs::write(staging.path().join("bin/holter-http-api"), b"mock").unwrap();

    let out = TempDir::new().unwrap();
    let result = run_bash(
        &script,
        &[
            "--staging",
            staging.path().to_str().unwrap(),
            "--output",
            out.path().to_str().unwrap(),
            "--prepare-only",
        ],
    );
    assert!(
        !result.status.success(),
        "fpm.sh must fail when NOTICE/ini are missing; stdout:\n{}",
        String::from_utf8_lossy(&result.stdout)
    );
}

/// Minimal recursive file walk without extra deps.
fn walkdir_files(dir: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(cur) = stack.pop() {
        let Ok(rd) = fs::read_dir(&cur) else {
            continue;
        };
        for entry in rd.flatten() {
            let p = entry.path();
            if p.is_dir() {
                stack.push(p);
            } else if p.is_file() {
                out.push(p);
            }
        }
    }
    out
}
