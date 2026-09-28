//! Shared helpers for integration tests. Include with `mod common;`.
//!
//! Each integration test is its own crate and typically uses only part of these
//! helpers, so dead-code warnings are allowed here to keep
//! `cargo clippy --all-targets -- -D warnings` green.
#![allow(dead_code)]

pub mod license_mock;
