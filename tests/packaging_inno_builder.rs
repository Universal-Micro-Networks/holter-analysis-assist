//! packaging-distribution task 2.2: InnoBuilder contract (file content checks).
//!
//! Inno Setup Compiler (ISCC) is Windows-only; CI/dev hosts without it still
//! assert `.iss` / optional build-script wiring rather than running a compile.

use std::fs;
use std::path::{Path, PathBuf};

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn read_utf8(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("read {}: {e}", path.display()))
}

fn strip_iss_comments(body: &str) -> String {
    body.lines()
        .filter(|l| {
            let t = l.trim();
            !t.is_empty() && !t.starts_with(';')
        })
        .collect::<Vec<_>>()
        .join("\n")
}

#[test]
fn iss_script_exists_and_targets_windows_x64() {
    let iss = repo_root().join("packaging/windows/holter-http-api.iss");
    assert!(
        iss.is_file(),
        "InnoBuilder: packaging/windows/holter-http-api.iss must exist at {}",
        iss.display()
    );
    let body = read_utf8(&iss);
    let active = strip_iss_comments(&body);
    let lower = active.to_ascii_lowercase();

    assert!(
        lower.contains("architecturesallowed=x64")
            || lower.contains("architecturesinstallin64bitmode=x64")
            || (lower.contains("x64compatible") && lower.contains("architectures")),
        "iss must target Windows x86_64 (ArchitecturesAllowed / InstallIn64BitMode x64); got:\n{body}"
    );
}

#[test]
fn iss_places_exe_sample_ini_notice_and_short_docs() {
    let body = read_utf8(&repo_root().join("packaging/windows/holter-http-api.iss"));
    let active = strip_iss_comments(&body);
    let lower = active.to_ascii_lowercase();

    assert!(
        lower.contains("[files]"),
        "iss must have a [Files] section; got:\n{body}"
    );

    // Staging Windows tree: bin/holter-http-api.exe
    assert!(
        active.contains("holter-http-api.exe"),
        "iss must install holter-http-api.exe; got:\n{body}"
    );

    // Sample ini (Req 6.1)
    assert!(
        active.contains("http.ini.example") || lower.contains("ini.example"),
        "iss must install sample ini (http.ini.example); got:\n{body}"
    );

    // NOTICE (Req 7.1)
    assert!(
        active.contains("NOTICE"),
        "iss must install NOTICE; got:\n{body}"
    );

    // Short docs from staging (design: 短縮ドキュメント)
    assert!(
        active.contains("docs") || active.contains("EMBEDDED_BINARY"),
        "iss must install short docs (docs/ or EMBEDDED_BINARY); got:\n{body}"
    );

    // Active file sources must not pull raw model weights (Req 4.1).
    assert!(
        !lower.contains(".onnx") && !lower.contains("resources\\models") && !lower.contains("resources/models"),
        "active iss instructions must not reference .onnx or resources/models; got:\n{active}"
    );
}

#[test]
fn iss_output_is_cpu_setup_exe_not_msi_or_service() {
    let body = read_utf8(&repo_root().join("packaging/windows/holter-http-api.iss"));
    let active = strip_iss_comments(&body);
    let lower = active.to_ascii_lowercase();

    // Design output: holter-http-api-setup-<version>-cpu.exe
    assert!(
        lower.contains("outputbasefilename")
            && active.contains("holter-http-api-setup-")
            && (active.contains("-cpu") || lower.contains("-cpu")),
        "iss OutputBaseFilename must be holter-http-api-setup-<version>-cpu; got:\n{body}"
    );

    // No Windows Service registration (task / design).
    assert!(
        !lower.contains("[services]")
            && !lower.contains("createservice")
            && !lower.contains("sc.exe")
            && !lower.contains("sc create"),
        "iss must not register a Windows Service; got:\n{body}"
    );

    // Not MSI / WiX.
    assert!(
        !lower.contains(".msi") && !lower.contains("wix") && !lower.contains("msiexec"),
        "iss must not produce or invoke MSI/WiX; got:\n{body}"
    );
}

#[test]
fn optional_build_script_fails_non_zero_when_iscc_missing_or_fails() {
    let root = repo_root();
    let script = root.join("packaging/scripts/build-inno.sh");
    assert!(
        script.is_file(),
        "optional Inno build helper must exist at {}",
        script.display()
    );
    let script_body = read_utf8(&script);

    // Compile failure / missing ISCC must not be treated as success (Req 2.4).
    assert!(
        script_body.contains("set -e") || script_body.contains("set -euo pipefail"),
        "build-inno.sh must use set -e (fail closed); got:\n{script_body}"
    );
    assert!(
        script_body.to_ascii_lowercase().contains("iscc")
            || script_body.contains("Inno Setup"),
        "build-inno.sh must invoke ISCC / Inno Setup; got:\n{script_body}"
    );
    assert!(
        script_body.contains("holter-http-api.iss"),
        "build-inno.sh must compile packaging/windows/holter-http-api.iss; got:\n{script_body}"
    );
    // Must propagate compiler failure (explicit exit or relying on set -e + iscc).
    assert!(
        script_body.contains("exit") || script_body.contains("exec "),
        "build-inno.sh must surface non-zero exit on compile failure; got:\n{script_body}"
    );
}
