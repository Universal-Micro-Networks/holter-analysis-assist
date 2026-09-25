//! packaging-distribution task 2.1: DockerBuilder contract (file content checks).
//!
//! Docker Engine may be unavailable in CI/dev hosts, so we assert Dockerfile /
//! .dockerignore / build-docker.sh wiring rather than running `docker build`.

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

#[test]
fn dockerfile_copies_staging_binary_ini_and_notice_on_bookworm_slim() {
    let dockerfile = repo_root().join("packaging/docker/Dockerfile");
    assert!(
        dockerfile.is_file(),
        "DockerBuilder: packaging/docker/Dockerfile must exist at {}",
        dockerfile.display()
    );
    let body = read_utf8(&dockerfile);
    let lower = body.to_ascii_lowercase();

    assert!(
        lower.contains("from debian:bookworm-slim"),
        "Dockerfile must use debian:bookworm-slim; got:\n{body}"
    );

    // Embedded binary from staging (COPY), not an in-image model-injection rebuild.
    assert!(
        body.contains("COPY")
            && (body.contains("holter-http-api") || lower.contains("holter-http-api")),
        "Dockerfile must COPY embedded holter-http-api from staging; got:\n{body}"
    );
    assert!(
        body.contains("COPY") && body.contains("NOTICE"),
        "Dockerfile must COPY NOTICE; got:\n{body}"
    );
    assert!(
        body.contains("COPY")
            && (body.contains("http.ini.example") || body.contains("ini.example")),
        "Dockerfile must COPY sample ini; got:\n{body}"
    );

    // No raw model files in image instructions (comments may mention exclusions).
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
        !active_lower.contains(".onnx") && !active_lower.contains("resources/models"),
        "active Dockerfile instructions must not reference .onnx or resources/models; got:\n{active}"
    );

    // --ignorefile is not a portable docker build flag; do not document it.
    assert!(
        !body.contains("--ignorefile"),
        "Dockerfile must not document invalid --ignorefile; wire .dockerignore into context instead; got:\n{body}"
    );
}

#[test]
fn dockerfile_documents_cpu_default_tag_and_config_port() {
    let body = read_utf8(&repo_root().join("packaging/docker/Dockerfile"));
    let lower = body.to_ascii_lowercase();

    // CPU default tag policy (comments / tag examples); CUDA is separate if any.
    assert!(
        (lower.contains("-cpu") || lower.contains("cpu"))
            && (lower.contains("tag") || lower.contains("holter-http-api:")),
        "Dockerfile must document CPU default image tag (e.g. holter-http-api:<version>-cpu); got:\n{body}"
    );

    // Config path / listen port documented for operators (Req 1.2 means).
    assert!(
        lower.contains("holter_http_ini")
            || lower.contains("--config")
            || lower.contains("config/http.ini"),
        "Dockerfile must document config path (--config / HOLTER_HTTP_INI / config/http.ini); got:\n{body}"
    );
    assert!(
        body.contains("8080") || lower.contains("expose"),
        "Dockerfile must document listen port (sample bind 8080 / EXPOSE); got:\n{body}"
    );

    // Prefer explicit linux/amd64 in build docs.
    assert!(
        body.contains("--platform linux/amd64") || body.contains("linux/amd64"),
        "Dockerfile docs must prefer --platform linux/amd64; got:\n{body}"
    );
}

#[test]
fn dockerignore_excludes_models_and_onnx() {
    let ignore = repo_root().join("packaging/docker/.dockerignore");
    assert!(
        ignore.is_file(),
        "DockerBuilder: packaging/docker/.dockerignore must exist at {}",
        ignore.display()
    );
    let body = read_utf8(&ignore);
    let lower = body.to_ascii_lowercase();

    assert!(
        lower.contains("resources/models") || lower.contains("**/models"),
        ".dockerignore must exclude resources/models; got:\n{body}"
    );
    assert!(
        lower.contains(".onnx"),
        ".dockerignore must exclude *.onnx; got:\n{body}"
    );
    assert!(
        !body.contains("--ignorefile"),
        ".dockerignore comments must not recommend invalid --ignorefile; got:\n{body}"
    );
}

#[test]
fn build_docker_script_wires_dockerignore_into_staging_context() {
    let root = repo_root();
    let script = root.join("packaging/scripts/build-docker.sh");
    let canonical = root.join("packaging/docker/.dockerignore");
    assert!(script.is_file(), "missing build helper: {}", script.display());
    assert!(
        canonical.is_file(),
        "missing canonical .dockerignore: {}",
        canonical.display()
    );

    let script_body = read_utf8(&script);
    assert!(
        script_body.contains("--platform linux/amd64"),
        "build-docker.sh must prefer --platform linux/amd64; got:\n{script_body}"
    );
    assert!(
        !script_body.contains("--ignorefile"),
        "build-docker.sh must not use --ignorefile; got:\n{script_body}"
    );
    assert!(
        script_body.contains(".dockerignore")
            && (script_body.contains("cp ") || script_body.contains("cp\t")),
        "build-docker.sh must copy .dockerignore into the build context; got:\n{script_body}"
    );

    // Exclusion must be wired into the *context* root (staging/.dockerignore).
    let staging = TempDir::new().unwrap();
    // Minimal staging tree so the script accepts the directory.
    fs::create_dir_all(staging.path().join("bin")).unwrap();
    fs::write(staging.path().join("bin/holter-http-api"), b"mock").unwrap();
    fs::write(staging.path().join("NOTICE"), "notice\n").unwrap();
    fs::write(staging.path().join("http.ini.example"), "[http]\nbind=0.0.0.0:8080\n").unwrap();

    let out = run_bash(
        &script,
        &[
            "--staging",
            staging.path().to_str().unwrap(),
            "--prepare-only",
        ],
    );
    assert!(
        out.status.success(),
        "build-docker.sh --prepare-only must exit 0; stderr:\n{}",
        String::from_utf8_lossy(&out.stderr)
    );

    let wired = staging.path().join(".dockerignore");
    assert!(
        wired.is_file(),
        "exclusion must be wired to context: expected {}",
        wired.display()
    );
    let wired_body = read_utf8(&wired);
    let canonical_body = read_utf8(&canonical);
    assert_eq!(
        wired_body, canonical_body,
        "staging/.dockerignore must match packaging/docker/.dockerignore"
    );
    assert!(
        wired_body.to_ascii_lowercase().contains(".onnx"),
        "wired context .dockerignore must still exclude *.onnx"
    );
}
