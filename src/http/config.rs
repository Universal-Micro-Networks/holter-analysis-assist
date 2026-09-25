//! HTTP listen / limit settings loaded from an `[http]` ini section.
//!
//! Canonical key meanings live in `config/http.ini.example` (this feature owns
//! that sample). `[license]` keys are owned by `config/license.ini.example`
//! and must not be redefined here.

use crate::phase2::ExecutionProviderKind;
use std::path::{Path, PathBuf};
use std::str::FromStr;
use std::time::Duration;
use thiserror::Error;

const SECTION: &str = "http";
const KEY_BIND: &str = "bind";
const KEY_MAX_BODY_BYTES: &str = "max_body_bytes";
const KEY_REQUEST_TIMEOUT_SECS: &str = "request_timeout_secs";
const KEY_MODEL_PATH: &str = "model_path";
const KEY_PROVIDER: &str = "provider";

/// Default max ECL body size: 512 MiB.
pub const DEFAULT_MAX_BODY_BYTES: usize = 536_870_912;
/// Default end-to-end request timeout: 1800 seconds (30 minutes).
pub const DEFAULT_REQUEST_TIMEOUT_SECS: u64 = 1800;

/// Failures while loading or validating `[http]` settings (fail-closed).
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum HttpConfigError {
    #[error("{0}")]
    Config(String),
}

/// Validated HTTP listen and limit settings from a `[http]` ini section.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HttpConfig {
    pub bind: String,
    pub max_body_bytes: usize,
    pub request_timeout: Duration,
    pub model_path: Option<PathBuf>,
    pub provider: ExecutionProviderKind,
}

impl HttpConfig {
    /// Load and validate `[http]` settings from `path`.
    ///
    /// Fail-closed: missing file, missing section/`bind`, or invalid values
    /// yield [`HttpConfigError::Config`].
    ///
    /// A colocated `[license]` section is ignored here; load it with the
    /// upstream [`crate::license::LicenseConfig`] using the same key names.
    pub fn load_from_path(path: &Path) -> Result<Self, HttpConfigError> {
        let ini = ini::Ini::load_from_file(path).map_err(|e| {
            HttpConfigError::Config(format!(
                "failed to read http ini {}: {e}",
                path.display()
            ))
        })?;

        let section = ini.section(Some(SECTION)).ok_or_else(|| {
            HttpConfigError::Config(format!(
                "missing [{SECTION}] section in {}",
                path.display()
            ))
        })?;

        let bind_raw = section.get(KEY_BIND).ok_or_else(|| {
            HttpConfigError::Config(format!(
                "missing required key '{KEY_BIND}' in [{SECTION}]"
            ))
        })?;
        let bind = validate_bind(bind_raw)?;

        let max_body_bytes = match section.get(KEY_MAX_BODY_BYTES) {
            Some(raw) => parse_max_body_bytes(raw)?,
            None => DEFAULT_MAX_BODY_BYTES,
        };

        let request_timeout = match section.get(KEY_REQUEST_TIMEOUT_SECS) {
            Some(raw) => parse_timeout_secs(raw)?,
            None => Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS),
        };

        let model_path = match section.get(KEY_MODEL_PATH) {
            Some(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return Err(config_err(format!(
                        "invalid '{KEY_MODEL_PATH}': value must not be empty when set"
                    )));
                }
                Some(PathBuf::from(trimmed))
            }
            None => None,
        };

        let provider = match section.get(KEY_PROVIDER) {
            Some(raw) => {
                let trimmed = raw.trim();
                if trimmed.is_empty() {
                    return Err(config_err(format!(
                        "invalid '{KEY_PROVIDER}': value must not be empty when set"
                    )));
                }
                ExecutionProviderKind::from_str(trimmed).map_err(|e| {
                    config_err(format!("invalid '{KEY_PROVIDER}': {e}"))
                })?
            }
            None => ExecutionProviderKind::Auto,
        };

        Ok(Self {
            bind,
            max_body_bytes,
            request_timeout,
            model_path,
            provider,
        })
    }
}

fn config_err(msg: impl Into<String>) -> HttpConfigError {
    HttpConfigError::Config(msg.into())
}

fn validate_bind(raw: &str) -> Result<String, HttpConfigError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(config_err(format!(
            "invalid '{KEY_BIND}': value must not be empty"
        )));
    }

    let Some((host, port)) = trimmed.rsplit_once(':') else {
        return Err(config_err(format!(
            "invalid '{KEY_BIND}': expected host:port, got '{trimmed}'"
        )));
    };
    if host.is_empty() {
        return Err(config_err(format!(
            "invalid '{KEY_BIND}': host must not be empty (got '{trimmed}')"
        )));
    }
    let port: u16 = port.parse().map_err(|_| {
        config_err(format!(
            "invalid '{KEY_BIND}': port must be a u16 integer, got '{port}'"
        ))
    })?;
    if port == 0 {
        return Err(config_err(format!(
            "invalid '{KEY_BIND}': port must be non-zero"
        )));
    }

    Ok(trimmed.to_string())
}

fn parse_max_body_bytes(raw: &str) -> Result<usize, HttpConfigError> {
    let trimmed = raw.trim();
    let bytes: usize = trimmed.parse().map_err(|_| {
        config_err(format!(
            "invalid '{KEY_MAX_BODY_BYTES}': expected positive integer, got '{trimmed}'"
        ))
    })?;
    if bytes == 0 {
        return Err(config_err(format!(
            "invalid '{KEY_MAX_BODY_BYTES}': must be a positive integer (got 0)"
        )));
    }
    Ok(bytes)
}

fn parse_timeout_secs(raw: &str) -> Result<Duration, HttpConfigError> {
    let trimmed = raw.trim();
    let secs: u64 = trimmed.parse().map_err(|_| {
        config_err(format!(
            "invalid '{KEY_REQUEST_TIMEOUT_SECS}': expected positive integer, got '{trimmed}'"
        ))
    })?;
    if secs == 0 {
        return Err(config_err(format!(
            "invalid '{KEY_REQUEST_TIMEOUT_SECS}': must be a positive integer (got 0)"
        )));
    }
    Ok(Duration::from_secs(secs))
}

#[cfg(test)]
mod tests {
    use super::{
        HttpConfig, HttpConfigError, DEFAULT_MAX_BODY_BYTES, DEFAULT_REQUEST_TIMEOUT_SECS,
        KEY_BIND, KEY_MAX_BODY_BYTES, KEY_MODEL_PATH, KEY_PROVIDER, KEY_REQUEST_TIMEOUT_SECS,
        SECTION,
    };
    use crate::phase2::ExecutionProviderKind;
    use std::io::Write;
    use std::time::Duration;
    use tempfile::NamedTempFile;

    fn write_ini(body: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().expect("temp ini");
        f.write_all(body.as_bytes()).expect("write ini");
        f
    }

    #[test]
    fn loads_required_bind_and_applies_size_timeout_defaults() {
        let ini = write_ini(
            r#"[http]
bind=0.0.0.0:8080
"#,
        );
        let cfg = HttpConfig::load_from_path(ini.path()).expect("valid minimal ini");
        assert_eq!(cfg.bind, "0.0.0.0:8080");
        assert_eq!(cfg.max_body_bytes, DEFAULT_MAX_BODY_BYTES);
        assert_eq!(
            cfg.request_timeout,
            Duration::from_secs(DEFAULT_REQUEST_TIMEOUT_SECS)
        );
        assert!(cfg.model_path.is_none());
        assert_eq!(cfg.provider, ExecutionProviderKind::Auto);
        assert_eq!(DEFAULT_MAX_BODY_BYTES, 512 * 1024 * 1024);
        assert_eq!(DEFAULT_REQUEST_TIMEOUT_SECS, 1800);
    }

    #[test]
    fn loads_optional_overrides_with_same_key_names() {
        // Key names are OS-agnostic string literals (Win/Linux identical).
        assert_eq!(SECTION, "http");
        assert_eq!(KEY_BIND, "bind");
        assert_eq!(KEY_MAX_BODY_BYTES, "max_body_bytes");
        assert_eq!(KEY_REQUEST_TIMEOUT_SECS, "request_timeout_secs");
        assert_eq!(KEY_MODEL_PATH, "model_path");
        assert_eq!(KEY_PROVIDER, "provider");

        let ini = write_ini(
            r#"[http]
bind=127.0.0.1:9090
max_body_bytes=1048576
request_timeout_secs=60
model_path=/tmp/dev-model.onnx
provider=cpu
"#,
        );
        let cfg = HttpConfig::load_from_path(ini.path()).expect("full ini");
        assert_eq!(cfg.bind, "127.0.0.1:9090");
        assert_eq!(cfg.max_body_bytes, 1_048_576);
        assert_eq!(cfg.request_timeout, Duration::from_secs(60));
        assert_eq!(
            cfg.model_path.as_deref(),
            Some(std::path::Path::new("/tmp/dev-model.onnx"))
        );
        assert_eq!(cfg.provider, ExecutionProviderKind::Cpu);
    }

    #[test]
    fn missing_bind_is_config_error_fail_closed() {
        let ini = write_ini(
            r#"[http]
max_body_bytes=1024
"#,
        );
        let err = HttpConfig::load_from_path(ini.path()).expect_err("missing bind");
        assert!(matches!(err, HttpConfigError::Config(_)));
        let msg = err.to_string();
        assert!(
            msg.contains("bind") || msg.to_lowercase().contains("missing"),
            "config error should mention missing bind: {msg}"
        );
    }

    #[test]
    fn missing_http_section_is_config_error() {
        let ini = write_ini(
            r#"[other]
bind=0.0.0.0:8080
"#,
        );
        let err = HttpConfig::load_from_path(ini.path()).expect_err("missing section");
        assert!(matches!(err, HttpConfigError::Config(_)));
    }

    #[test]
    fn missing_file_is_config_error() {
        let err = HttpConfig::load_from_path(std::path::Path::new(
            "/nonexistent/http-api-config-test.ini",
        ))
        .expect_err("missing file");
        assert!(matches!(err, HttpConfigError::Config(_)));
    }

    #[test]
    fn empty_bind_is_config_error() {
        let ini = write_ini(
            r#"[http]
bind=
"#,
        );
        let err = HttpConfig::load_from_path(ini.path()).expect_err("empty bind");
        assert!(matches!(err, HttpConfigError::Config(_)));
    }

    #[test]
    fn invalid_bind_without_port_is_config_error() {
        let ini = write_ini(
            r#"[http]
bind=not-a-listen-addr
"#,
        );
        let err = HttpConfig::load_from_path(ini.path()).expect_err("bad bind");
        assert!(matches!(err, HttpConfigError::Config(_)));
    }

    #[test]
    fn non_positive_timeout_is_config_error() {
        let ini = write_ini(
            r#"[http]
bind=0.0.0.0:8080
request_timeout_secs=0
"#,
        );
        let err = HttpConfig::load_from_path(ini.path()).expect_err("timeout 0");
        assert!(matches!(err, HttpConfigError::Config(_)));
    }

    #[test]
    fn invalid_timeout_is_config_error() {
        let ini = write_ini(
            r#"[http]
bind=0.0.0.0:8080
request_timeout_secs=abc
"#,
        );
        let err = HttpConfig::load_from_path(ini.path()).expect_err("bad timeout");
        assert!(matches!(err, HttpConfigError::Config(_)));
    }

    #[test]
    fn invalid_max_body_bytes_is_config_error() {
        let ini = write_ini(
            r#"[http]
bind=0.0.0.0:8080
max_body_bytes=0
"#,
        );
        let err = HttpConfig::load_from_path(ini.path()).expect_err("body 0");
        assert!(matches!(err, HttpConfigError::Config(_)));
    }

    #[test]
    fn invalid_provider_is_config_error() {
        let ini = write_ini(
            r#"[http]
bind=0.0.0.0:8080
provider=not-a-provider
"#,
        );
        let err = HttpConfig::load_from_path(ini.path()).expect_err("bad provider");
        assert!(matches!(err, HttpConfigError::Config(_)));
    }

    #[test]
    fn colocated_license_section_does_not_redefine_license_keys() {
        // Req 6.2: [license] may share the file; HttpConfig must not invent
        // alternate license key names. Upstream LicenseConfig still loads.
        let ini = write_ini(
            r#"[http]
bind=0.0.0.0:8080

[license]
server_url=https://license.example.com
"#,
        );
        let http = HttpConfig::load_from_path(ini.path()).expect("http section");
        assert_eq!(http.bind, "0.0.0.0:8080");

        let license = crate::license::LicenseConfig::load_from_path(ini.path())
            .expect("upstream license keys unchanged");
        assert_eq!(license.server_url, "https://license.example.com");
    }

    #[test]
    fn example_ini_documents_http_keys_tls_and_license_canonical() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/http.ini.example");
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("canonical sample missing at {}: {e}", path.display()));
        assert!(
            body.contains("[http]"),
            "example must define [http] section"
        );
        for key in [
            "bind",
            "max_body_bytes",
            "request_timeout_secs",
            "model_path",
            "provider",
        ] {
            assert!(
                body.contains(key),
                "example must document key '{key}'"
            );
        }
        let lower = body.to_lowercase();
        assert!(
            lower.contains("tls")
                || body.contains("TLS")
                || body.contains("リバースプロキシ")
                || lower.contains("reverse proxy"),
            "example must note TLS termination / reverse-proxy ops: {body}"
        );
        assert!(
            body.contains("license.ini.example")
                || body.contains("[license]")
                || body.contains("正本"),
            "example must reference upstream [license] canonical, not redefine keys: {body}"
        );
        // Must not redefine license key meanings as http-owned.
        assert!(
            !body.contains("server_url=") || body.contains("license.ini.example"),
            "if server_url appears, it must be via license reference/merge guidance"
        );
        // Task 5.3: manual smoke notes for real server / release-embedded-http-api.
        assert!(
            body.contains("release-embedded-http-api"),
            "example must document manual smoke using release-embedded-http-api artifact"
        );
        assert!(
            body.contains("/health") && body.contains("/v1/analyze"),
            "manual smoke notes must mention health and analyze checks"
        );
    }
}
