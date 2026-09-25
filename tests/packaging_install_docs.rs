//! packaging-distribution task 4.1: InstallDocs (3 系統).
//!
//! docs/packaging/{docker,windows,linux-packages}.md が存在し、
//! 導入・起動、ini 編集、NOTICE 所在、対象 OS、CPU 既定／CUDA 任意を追えること。

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_utf8(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn assert_common_operator_content(body: &str, path_label: &str) {
    let lower = body.to_ascii_lowercase();

    // Sample ini edit: license URL + listen (Req 6.4, 9.2; design InstallDocs).
    assert!(
        body.contains("server_url") && (body.contains("bind") || lower.contains("リッスン")),
        "{path_label}: must document sample ini edit (server_url + bind/listen); got:\n{body}"
    );
    assert!(
        lower.contains("http.ini") || lower.contains("ini.example") || body.contains("サンプル"),
        "{path_label}: must mention sample ini path or copy step; got:\n{body}"
    );

    // NOTICE location (Req 7.2).
    assert!(
        body.contains("NOTICE"),
        "{path_label}: must document NOTICE location; got:\n{body}"
    );

    // Target OS/arch limited to Win + Linux x86_64 (Req 9.3).
    assert!(
        (body.contains("x86_64") || body.contains("amd64"))
            && (lower.contains("windows") || body.contains("Windows"))
            && (lower.contains("linux") || body.contains("Linux")),
        "{path_label}: must state Windows/Linux x86_64 scope; got:\n{body}"
    );

    // CPU default / CUDA optional (Req 5.4).
    assert!(
        (lower.contains("cpu") || body.contains("CPU"))
            && (lower.contains("cuda") || body.contains("CUDA")),
        "{path_label}: must document CPU default and CUDA optional; got:\n{body}"
    );

    // Japanese operator docs (spec.json language=ja).
    assert!(
        body.contains("導入") || body.contains("起動") || body.contains("設定"),
        "{path_label}: docs must be Japanese (導入/起動/設定); got:\n{body}"
    );

    // Boundary: do not redesign API / license server here (Req 10.x).
    assert!(
        body.contains("上流")
            || lower.contains("license-client")
            || lower.contains("http-api")
            || body.contains("別プロジェクト")
            || body.contains("範囲外"),
        "{path_label}: must defer API/license-server details upstream; got:\n{body}"
    );
}

#[test]
fn install_docs_docker_exists_and_covers_operator_topics() {
    let path = repo_root().join("docs/packaging/docker.md");
    assert!(
        path.is_file(),
        "InstallDocs: docs/packaging/docker.md must exist at {}",
        path.display()
    );
    let body = read_utf8(&path);
    assert_common_operator_content(&body, "docker.md");
    let lower = body.to_ascii_lowercase();
    assert!(
        lower.contains("docker") || body.contains("コンテナ") || body.contains("イメージ"),
        "docker.md must describe container image install/start; got:\n{body}"
    );
    assert!(
        lower.contains("8080") || lower.contains("port") || body.contains("ポート"),
        "docker.md must mention listen/port mapping; got:\n{body}"
    );
}

#[test]
fn install_docs_windows_exists_and_covers_operator_topics() {
    let path = repo_root().join("docs/packaging/windows.md");
    assert!(
        path.is_file(),
        "InstallDocs: docs/packaging/windows.md must exist at {}",
        path.display()
    );
    let body = read_utf8(&path);
    assert_common_operator_content(&body, "windows.md");
    let lower = body.to_ascii_lowercase();
    assert!(
        lower.contains("installer")
            || lower.contains("setup")
            || body.contains("インストーラ")
            || body.contains("インストール"),
        "windows.md must describe installer install/start; got:\n{body}"
    );
    assert!(
        body.contains("holter-http-api.exe") || lower.contains(".exe"),
        "windows.md must mention Windows binary; got:\n{body}"
    );
}

#[test]
fn install_docs_linux_packages_exists_and_covers_operator_topics() {
    let path = repo_root().join("docs/packaging/linux-packages.md");
    assert!(
        path.is_file(),
        "InstallDocs: docs/packaging/linux-packages.md must exist at {}",
        path.display()
    );
    let body = read_utf8(&path);
    assert_common_operator_content(&body, "linux-packages.md");
    assert!(
        body.contains("deb") && body.contains("rpm"),
        "linux-packages.md must cover both deb and rpm; got:\n{body}"
    );
    assert!(
        body.contains("/usr/bin/holter-http-api")
            || body.contains("/usr/share/holter-http-api"),
        "linux-packages.md must document package install paths; got:\n{body}"
    );
}
