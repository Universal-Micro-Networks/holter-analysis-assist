//! packaging-distribution task 3.1: ReleasePublish CI contract.
//!
//! Runs tools/check_ci_packaging_release.sh against .github/workflows/ci.yml
//! (same pattern as tools/check_ci_embed_*.sh for embed release jobs).

use std::path::PathBuf;
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

#[test]
fn check_ci_packaging_release_contract_passes() {
    let script = repo_root().join("tools/check_ci_packaging_release.sh");
    assert!(
        script.is_file(),
        "ReleasePublish check script must exist at {}",
        script.display()
    );

    let output = Command::new("bash")
        .arg(&script)
        .current_dir(repo_root())
        .output()
        .unwrap_or_else(|e| panic!("run {}: {e}", script.display()));

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "check_ci_packaging_release.sh must pass (ReleasePublish CI contract).\nstdout:\n{stdout}\nstderr:\n{stderr}"
    );
}
