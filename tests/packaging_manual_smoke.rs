//! packaging-distribution task 4.3: 実バイナリ配布の手動スモーク観点（CI 必須外）.
//!
//! 運用者が追えるチェックリストがリポジトリ内に残っていることのみを自動確認する。
//! 実ライセンス環境・実埋め込みバイナリでの起動は初期 CI 必須にしない。

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_utf8(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

#[test]
fn manual_smoke_notes_exist_and_list_operator_checkpoints() {
    let path = repo_root().join("docs/packaging/manual-smoke.md");
    assert!(
        path.is_file(),
        "manual smoke notes must exist at {}",
        path.display()
    );
    let body = read_utf8(&path);
    let lower = body.to_ascii_lowercase();

    // Not CI-required (task 4.3).
    assert!(
        body.contains("CI")
            && (body.contains("必須ではない")
                || body.contains("必須にしない")
                || lower.contains("not required")
                || body.contains("任意")),
        "manual-smoke.md must state these checks are not CI-required; got:\n{body}"
    );

    // Embedded HTTP binary premise when available.
    assert!(
        body.contains("埋め込み") || lower.contains("embedded") || body.contains("release-embedded-http-api"),
        "manual-smoke.md must mention embedded HTTP binary premise; got:\n{body}"
    );

    // Four packaging smoke viewpoints (Req 1.2, 2.2, 3.3, 9.1).
    assert!(
        lower.contains("docker") || body.contains("コンテナ") || body.contains("イメージ"),
        "manual-smoke.md must cover image start; got:\n{body}"
    );
    assert!(
        body.contains("インストーラ") || lower.contains("installer") || lower.contains("windows"),
        "manual-smoke.md must cover Windows installer install; got:\n{body}"
    );
    assert!(
        (body.contains("deb") || body.contains("rpm") || body.contains("パッケージ"))
            && (lower.contains("linux") || body.contains("Linux")),
        "manual-smoke.md must cover Linux package install; got:\n{body}"
    );
    assert!(
        (body.contains("ini") || body.contains("設定"))
            && (body.contains("起動") || lower.contains("start") || lower.contains("run")),
        "manual-smoke.md must cover start after ini configuration; got:\n{body}"
    );
}
