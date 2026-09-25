//! packaging-distribution task 1.2 + 4.2: StagingLayout + ArtifactVerify.
//!
//! - prepare-staging assembles bin + NOTICE + sample ini under packaging/out/staging/<os>
//! - verify-artifact exits 0 on a valid tree
//! - verify-artifact exits non-zero on missing NOTICE / ini / binary, or forbidden model files
//!   (task 4.2: success + NOTICE/ini missing + forbidden ext including .onnx / .ort / .weights.h5)

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

use tempfile::TempDir;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn run_bash(script: &Path, args: &[&str]) -> std::process::Output {
    Command::new("bash")
        .arg(script)
        .args(args)
        .current_dir(repo_root())
        .output()
        .unwrap_or_else(|e| panic!("run {}: {e}", script.display()))
}

fn write_mock_binary(dir: &Path, name: &str) -> PathBuf {
    let path = dir.join(name);
    fs::write(&path, b"#!/bin/sh\necho mock-holter-http-api\n")
        .unwrap_or_else(|e| panic!("write mock binary {}: {e}", path.display()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mut perms = fs::metadata(&path).unwrap().permissions();
        perms.set_mode(0o755);
        fs::set_permissions(&path, perms).unwrap();
    }
    path
}

fn assert_staging_layout(staging: &Path, binary_name: &str) {
    assert!(
        staging.join("NOTICE").is_file(),
        "NOTICE missing under {}",
        staging.display()
    );
    assert!(
        staging.join("http.ini.example").is_file(),
        "sample ini missing under {}",
        staging.display()
    );
    let bin = staging.join("bin").join(binary_name);
    assert!(bin.is_file(), "binary missing at {}", bin.display());
    let ini = fs::read_to_string(staging.join("http.ini.example")).unwrap();
    assert!(
        ini.contains("[http]") && ini.contains("[license]"),
        "staging sample ini must include [http] and [license]"
    );
    let notice = fs::read_to_string(staging.join("NOTICE")).unwrap();
    assert!(!notice.trim().is_empty(), "NOTICE must be non-empty");
}

#[test]
fn prepare_staging_assembles_linux_layout_and_verify_passes() {
    let root = repo_root();
    let prepare = root.join("packaging/scripts/prepare-staging.sh");
    let verify = root.join("packaging/scripts/verify-artifact.sh");
    assert!(
        prepare.is_file(),
        "StagingLayout script missing: {}",
        prepare.display()
    );
    assert!(
        verify.is_file(),
        "ArtifactVerify script missing: {}",
        verify.display()
    );

    let tmp = TempDir::new().unwrap();
    let binary = write_mock_binary(tmp.path(), "holter-http-api");

    let out = run_bash(
        &prepare,
        &["--os", "linux", "--binary", binary.to_str().unwrap()],
    );
    assert!(
        out.status.success(),
        "prepare-staging.sh must exit 0; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let staging = root.join("packaging/out/staging/linux");
    assert_staging_layout(&staging, "holter-http-api");

    // Layout contract: packaging consumes embedded HTTP binary (not raw models).
    let readme = fs::read_to_string(root.join("packaging/staging/README.md")).unwrap();
    let lower = readme.to_ascii_lowercase();
    assert!(
        lower.contains("embedded") || lower.contains("埋め込み"),
        "staging README must state embedded-binary premise; got:\n{readme}"
    );
    assert!(
        !staging.join("bin").join("holter-http-api").extension().is_some_and(|e| e == "onnx"),
        "staging must not place .onnx as the binary"
    );

    let vout = run_bash(&verify, &[staging.to_str().unwrap()]);
    assert!(
        vout.status.success(),
        "verify-artifact.sh must exit 0 on valid staging; stderr:\n{}",
        String::from_utf8_lossy(&vout.stderr)
    );
}

#[test]
fn prepare_staging_assembles_windows_layout_with_exe() {
    let root = repo_root();
    let prepare = root.join("packaging/scripts/prepare-staging.sh");
    assert!(prepare.is_file(), "missing {}", prepare.display());

    let tmp = TempDir::new().unwrap();
    let binary = write_mock_binary(tmp.path(), "holter-http-api.exe");

    let out = run_bash(
        &prepare,
        &[
            "--os",
            "windows",
            "--binary",
            binary.to_str().unwrap(),
        ],
    );
    assert!(
        out.status.success(),
        "prepare-staging windows must exit 0; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let staging = root.join("packaging/out/staging/windows");
    assert_staging_layout(&staging, "holter-http-api.exe");
}

#[test]
fn verify_fails_when_notice_missing() {
    let root = repo_root();
    let verify = root.join("packaging/scripts/verify-artifact.sh");
    assert!(verify.is_file(), "missing {}", verify.display());

    let staging = TempDir::new().unwrap();
    fs::create_dir_all(staging.path().join("bin")).unwrap();
    fs::write(staging.path().join("bin/holter-http-api"), b"mock").unwrap();
    fs::write(
        staging.path().join("http.ini.example"),
        "[http]\nbind=0.0.0.0:8080\n\n[license]\nserver_url=https://x\n",
    )
    .unwrap();
    // NOTICE intentionally omitted

    let out = run_bash(&verify, &[staging.path().to_str().unwrap()]);
    assert!(
        !out.status.success(),
        "verify must fail without NOTICE; stdout:\n{}",
        String::from_utf8_lossy(&out.stdout)
    );
    let err = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        err.to_ascii_lowercase().contains("notice"),
        "failure reason should mention NOTICE; got:\n{err}"
    );
}

#[test]
fn verify_fails_when_sample_ini_missing() {
    let root = repo_root();
    let verify = root.join("packaging/scripts/verify-artifact.sh");
    assert!(verify.is_file(), "missing {}", verify.display());

    let staging = TempDir::new().unwrap();
    fs::create_dir_all(staging.path().join("bin")).unwrap();
    fs::write(staging.path().join("bin/holter-http-api"), b"mock").unwrap();
    fs::write(staging.path().join("NOTICE"), "ORT notice\n").unwrap();
    // http.ini.example intentionally omitted

    let out = run_bash(&verify, &[staging.path().to_str().unwrap()]);
    assert!(
        !out.status.success(),
        "verify must fail without sample ini"
    );
    let err = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let lower = err.to_ascii_lowercase();
    assert!(
        lower.contains("ini") || lower.contains("http.ini"),
        "failure reason should mention ini; got:\n{err}"
    );
}

#[test]
fn verify_fails_when_binary_missing() {
    let root = repo_root();
    let verify = root.join("packaging/scripts/verify-artifact.sh");
    assert!(verify.is_file(), "missing {}", verify.display());

    let staging = TempDir::new().unwrap();
    fs::write(staging.path().join("NOTICE"), "ORT notice\n").unwrap();
    fs::write(
        staging.path().join("http.ini.example"),
        "[http]\nbind=0.0.0.0:8080\n\n[license]\nserver_url=https://x\n",
    )
    .unwrap();
    // bin/holter-http-api intentionally omitted

    let out = run_bash(&verify, &[staging.path().to_str().unwrap()]);
    assert!(
        !out.status.success(),
        "verify must fail without binary"
    );
    let err = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let lower = err.to_ascii_lowercase();
    assert!(
        lower.contains("holter-http-api") || lower.contains("binary"),
        "failure reason should mention binary; got:\n{err}"
    );
}

#[test]
fn verify_fails_when_onnx_present() {
    let root = repo_root();
    let verify = root.join("packaging/scripts/verify-artifact.sh");
    assert!(verify.is_file(), "missing {}", verify.display());

    let staging = TempDir::new().unwrap();
    fs::create_dir_all(staging.path().join("bin")).unwrap();
    fs::write(staging.path().join("bin/holter-http-api"), b"mock").unwrap();
    fs::write(staging.path().join("NOTICE"), "ORT notice\n").unwrap();
    fs::write(
        staging.path().join("http.ini.example"),
        "[http]\nbind=0.0.0.0:8080\n\n[license]\nserver_url=https://x\n",
    )
    .unwrap();
    fs::write(staging.path().join("model.onnx"), b"raw-model").unwrap();

    let out = run_bash(&verify, &[staging.path().to_str().unwrap()]);
    assert!(
        !out.status.success(),
        "verify must fail when .onnx is present"
    );
    let err = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        err.to_ascii_lowercase().contains("onnx"),
        "failure reason should mention onnx; got:\n{err}"
    );
}

/// task 4.2 gap: forbidden extensions beyond `.onnx` must also fail closed.
#[test]
fn verify_fails_when_ort_present() {
    let root = repo_root();
    let verify = root.join("packaging/scripts/verify-artifact.sh");
    assert!(verify.is_file(), "missing {}", verify.display());

    let staging = TempDir::new().unwrap();
    fs::create_dir_all(staging.path().join("bin")).unwrap();
    fs::write(staging.path().join("bin/holter-http-api"), b"mock").unwrap();
    fs::write(staging.path().join("NOTICE"), "ORT notice\n").unwrap();
    fs::write(
        staging.path().join("http.ini.example"),
        "[http]\nbind=0.0.0.0:8080\n\n[license]\nserver_url=https://x\n",
    )
    .unwrap();
    fs::write(staging.path().join("weights.ort"), b"raw-ort").unwrap();

    let out = run_bash(&verify, &[staging.path().to_str().unwrap()]);
    assert!(
        !out.status.success(),
        "verify must fail when .ort is present"
    );
    let err = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    assert!(
        err.to_ascii_lowercase().contains("ort") || err.to_ascii_lowercase().contains("forbidden"),
        "failure reason should mention ort/forbidden; got:\n{err}"
    );
}

/// task 4.2 gap: `*.weights.h5` must fail closed.
#[test]
fn verify_fails_when_weights_h5_present() {
    let root = repo_root();
    let verify = root.join("packaging/scripts/verify-artifact.sh");
    assert!(verify.is_file(), "missing {}", verify.display());

    let staging = TempDir::new().unwrap();
    fs::create_dir_all(staging.path().join("bin")).unwrap();
    fs::write(staging.path().join("bin/holter-http-api"), b"mock").unwrap();
    fs::write(staging.path().join("NOTICE"), "ORT notice\n").unwrap();
    fs::write(
        staging.path().join("http.ini.example"),
        "[http]\nbind=0.0.0.0:8080\n\n[license]\nserver_url=https://x\n",
    )
    .unwrap();
    fs::write(staging.path().join("model.weights.h5"), b"raw-h5").unwrap();

    let out = run_bash(&verify, &[staging.path().to_str().unwrap()]);
    assert!(
        !out.status.success(),
        "verify must fail when .weights.h5 is present"
    );
    let err = format!(
        "{}{}",
        String::from_utf8_lossy(&out.stderr),
        String::from_utf8_lossy(&out.stdout)
    );
    let lower = err.to_ascii_lowercase();
    assert!(
        lower.contains("weights.h5") || lower.contains("forbidden") || lower.contains("h5"),
        "failure reason should mention weights.h5/forbidden; got:\n{err}"
    );
}
