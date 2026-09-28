//! Shared helpers for integration tests. Include with `mod common;`.
//!
//! Each integration test is its own crate and typically uses only part of these
//! helpers, so dead-code warnings are allowed here to keep
//! `cargo clippy --all-targets -- -D warnings` green.
#![allow(dead_code)]

pub mod license_mock;

use std::path::PathBuf;
use std::process::Command;

/// `bash` for running repository shell scripts.
///
/// On Windows a bare `bash` resolves to the System32 WSL launcher before PATH,
/// so Git Bash is picked from PATH instead.
pub fn bash() -> Command {
    Command::new(bash_program())
}

#[cfg(not(windows))]
fn bash_program() -> PathBuf {
    PathBuf::from("bash")
}

#[cfg(windows)]
fn bash_program() -> PathBuf {
    let is_wsl_launcher_dir = |dir: &std::path::Path| {
        let dir = dir.to_string_lossy().to_ascii_lowercase();
        dir.contains("system32") || dir.contains("windowsapps")
    };
    std::env::var_os("PATH")
        .iter()
        .flat_map(std::env::split_paths)
        .filter(|dir| !is_wsl_launcher_dir(dir))
        .map(|dir| dir.join("bash.exe"))
        .chain(std::iter::once(PathBuf::from(
            r"C:\Program Files\Git\bin\bash.exe",
        )))
        .find(|p| p.is_file())
        .unwrap_or_else(|| PathBuf::from("bash"))
}
