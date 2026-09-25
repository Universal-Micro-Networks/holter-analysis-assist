//! License connection settings loaded from a `[license]` ini section.
//!
//! Canonical key meanings and defaults live in `config/license.ini.example`
//! (this feature owns that sample; other features must not redefine keys).

use super::types::LicenseError;
use std::fmt;
use std::path::Path;
use std::time::Duration;

const SECTION: &str = "license";
const KEY_SERVER_URL: &str = "server_url";
const KEY_API_KEY: &str = "api_key";
const KEY_TIMEOUT_SECS: &str = "timeout_secs";
const KEY_CHECK_PATH: &str = "check_path";
const KEY_METER_PATH: &str = "meter_path";

const DEFAULT_TIMEOUT_SECS: u64 = 10;
const DEFAULT_CHECK_PATH: &str = "/v1/license/check";
const DEFAULT_METER_PATH: &str = "/v1/license/meter";

/// Opaque string that never prints its plaintext in [`Debug`].
#[derive(Clone, PartialEq, Eq)]
pub struct SecretString(String);

impl SecretString {
    /// Wrap a secret value.
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }

    /// Borrow the plaintext (for Authorization headers, etc.). Prefer not logging this.
    pub fn expose_secret(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for SecretString {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("***")
    }
}

/// Validated license-server connection settings from a `[license]` ini section.
#[derive(Clone, PartialEq, Eq)]
pub struct LicenseConfig {
    pub server_url: String,
    pub api_key: Option<SecretString>,
    pub timeout: Duration,
    pub check_path: String,
    pub meter_path: String,
}

impl fmt::Debug for LicenseConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LicenseConfig")
            .field("server_url", &self.server_url)
            .field("api_key", &self.api_key)
            .field("timeout", &self.timeout)
            .field("check_path", &self.check_path)
            .field("meter_path", &self.meter_path)
            .finish()
    }
}

impl LicenseConfig {
    /// Load and validate `[license]` settings from `path`.
    ///
    /// Fail-closed: missing file, missing section/keys, invalid URL, or non-positive
    /// timeout yield [`LicenseError::Config`].
    pub fn load_from_path(path: &Path) -> Result<Self, LicenseError> {
        let ini = ini::Ini::load_from_file(path).map_err(|e| {
            LicenseError::Config(format!("failed to read license ini {}: {e}", path.display()))
        })?;

        let section = ini.section(Some(SECTION)).ok_or_else(|| {
            LicenseError::Config(format!("missing [{SECTION}] section in {}", path.display()))
        })?;

        let server_url_raw = section.get(KEY_SERVER_URL).ok_or_else(|| {
            LicenseError::Config(format!("missing required key '{KEY_SERVER_URL}' in [{SECTION}]"))
        })?;
        let server_url = validate_server_url(server_url_raw)?;

        let api_key = match section.get(KEY_API_KEY) {
            Some(v) if !v.trim().is_empty() => Some(SecretString::new(v.trim())),
            _ => None,
        };

        let timeout = match section.get(KEY_TIMEOUT_SECS) {
            Some(raw) => parse_timeout_secs(raw)?,
            None => Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        };

        let check_path = optional_path(section.get(KEY_CHECK_PATH), DEFAULT_CHECK_PATH, KEY_CHECK_PATH)?;
        let meter_path = optional_path(section.get(KEY_METER_PATH), DEFAULT_METER_PATH, KEY_METER_PATH)?;

        Ok(Self {
            server_url,
            api_key,
            timeout,
            check_path,
            meter_path,
        })
    }
}

fn config_err(msg: impl Into<String>) -> LicenseError {
    LicenseError::Config(msg.into())
}

fn validate_server_url(raw: &str) -> Result<String, LicenseError> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err(config_err(format!(
            "invalid '{KEY_SERVER_URL}': value must not be empty"
        )));
    }

    let url = reqwest::Url::parse(trimmed).map_err(|e| {
        config_err(format!("invalid '{KEY_SERVER_URL}': not a valid URL ({e})"))
    })?;

    match url.scheme() {
        "https" | "http" => {}
        other => {
            return Err(config_err(format!(
                "invalid '{KEY_SERVER_URL}': unsupported scheme '{other}' (use https or http)"
            )));
        }
    }

    if url.host_str().is_none() {
        return Err(config_err(format!(
            "invalid '{KEY_SERVER_URL}': URL must include a host"
        )));
    }

    Ok(trimmed.to_string())
}

fn parse_timeout_secs(raw: &str) -> Result<Duration, LicenseError> {
    let trimmed = raw.trim();
    let secs: u64 = trimmed.parse().map_err(|_| {
        config_err(format!(
            "invalid '{KEY_TIMEOUT_SECS}': expected positive integer, got '{trimmed}'"
        ))
    })?;
    if secs == 0 {
        return Err(config_err(format!(
            "invalid '{KEY_TIMEOUT_SECS}': must be a positive integer (got 0)"
        )));
    }
    Ok(Duration::from_secs(secs))
}

fn optional_path(
    raw: Option<&str>,
    default: &str,
    key: &str,
) -> Result<String, LicenseError> {
    match raw {
        None => Ok(default.to_string()),
        Some(v) => {
            let trimmed = v.trim();
            if trimmed.is_empty() {
                return Err(config_err(format!(
                    "invalid '{key}': value must not be empty when set"
                )));
            }
            Ok(trimmed.to_string())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        LicenseConfig, SecretString, DEFAULT_CHECK_PATH, DEFAULT_METER_PATH, DEFAULT_TIMEOUT_SECS,
        KEY_API_KEY, KEY_CHECK_PATH, KEY_METER_PATH, KEY_SERVER_URL, KEY_TIMEOUT_SECS, SECTION,
    };
    use crate::license::LicenseError;
    use std::io::Write;
    use std::time::Duration;
    use tempfile::NamedTempFile;

    fn write_ini(body: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().expect("temp ini");
        f.write_all(body.as_bytes()).expect("write ini");
        f
    }

    #[test]
    fn loads_required_server_url_and_optional_keys_with_defaults() {
        let ini = write_ini(
            r#"[license]
server_url=https://license.example.com
"#,
        );
        let cfg = LicenseConfig::load_from_path(ini.path()).expect("valid minimal ini");
        assert_eq!(cfg.server_url, "https://license.example.com");
        assert!(cfg.api_key.is_none());
        assert_eq!(cfg.timeout, Duration::from_secs(DEFAULT_TIMEOUT_SECS));
        assert_eq!(cfg.check_path, DEFAULT_CHECK_PATH);
        assert_eq!(cfg.meter_path, DEFAULT_METER_PATH);
    }

    #[test]
    fn loads_optional_auth_timeout_and_paths_with_same_key_names() {
        // Key names are OS-agnostic string literals (Win/Linux identical).
        assert_eq!(KEY_SERVER_URL, "server_url");
        assert_eq!(KEY_API_KEY, "api_key");
        assert_eq!(KEY_TIMEOUT_SECS, "timeout_secs");
        assert_eq!(KEY_CHECK_PATH, "check_path");
        assert_eq!(KEY_METER_PATH, "meter_path");
        assert_eq!(SECTION, "license");

        let ini = write_ini(
            r#"[license]
server_url=http://127.0.0.1:9000
api_key=super-secret-token
timeout_secs=30
check_path=/custom/check
meter_path=/custom/meter
"#,
        );
        let cfg = LicenseConfig::load_from_path(ini.path()).expect("full ini");
        assert_eq!(cfg.server_url, "http://127.0.0.1:9000");
        assert_eq!(
            cfg.api_key.as_ref().map(SecretString::expose_secret),
            Some("super-secret-token")
        );
        assert_eq!(cfg.timeout, Duration::from_secs(30));
        assert_eq!(cfg.check_path, "/custom/check");
        assert_eq!(cfg.meter_path, "/custom/meter");
    }

    #[test]
    fn missing_server_url_is_config_error_fail_closed() {
        let ini = write_ini(
            r#"[license]
timeout_secs=5
"#,
        );
        let err = LicenseConfig::load_from_path(ini.path()).expect_err("missing server_url");
        assert!(matches!(err, LicenseError::Config(_)));
        let msg = err.to_string();
        assert!(
            msg.contains("server_url") || msg.to_lowercase().contains("missing"),
            "config error should mention missing server_url: {msg}"
        );
    }

    #[test]
    fn missing_license_section_is_config_error() {
        let ini = write_ini(
            r#"[other]
server_url=https://license.example.com
"#,
        );
        let err = LicenseConfig::load_from_path(ini.path()).expect_err("missing section");
        assert!(matches!(err, LicenseError::Config(_)));
    }

    #[test]
    fn missing_file_is_config_error() {
        let err = LicenseConfig::load_from_path(std::path::Path::new(
            "/nonexistent/license-client-test.ini",
        ))
        .expect_err("missing file");
        assert!(matches!(err, LicenseError::Config(_)));
    }

    #[test]
    fn invalid_server_url_is_config_error() {
        let ini = write_ini(
            r#"[license]
server_url=not-a-url
"#,
        );
        let err = LicenseConfig::load_from_path(ini.path()).expect_err("bad url");
        assert!(matches!(err, LicenseError::Config(_)));
    }

    #[test]
    fn empty_server_url_is_config_error() {
        let ini = write_ini(
            r#"[license]
server_url=
"#,
        );
        let err = LicenseConfig::load_from_path(ini.path()).expect_err("empty url");
        assert!(matches!(err, LicenseError::Config(_)));
    }

    #[test]
    fn non_positive_timeout_is_config_error() {
        let ini = write_ini(
            r#"[license]
server_url=https://license.example.com
timeout_secs=0
"#,
        );
        let err = LicenseConfig::load_from_path(ini.path()).expect_err("timeout 0");
        assert!(matches!(err, LicenseError::Config(_)));
    }

    #[test]
    fn invalid_timeout_is_config_error() {
        let ini = write_ini(
            r#"[license]
server_url=https://license.example.com
timeout_secs=abc
"#,
        );
        let err = LicenseConfig::load_from_path(ini.path()).expect_err("bad timeout");
        assert!(matches!(err, LicenseError::Config(_)));
    }

    #[test]
    fn api_key_is_masked_in_debug() {
        let secret = "plain-api-key-value-should-not-leak";
        let cfg = LicenseConfig {
            server_url: "https://license.example.com".into(),
            api_key: Some(SecretString::new(secret)),
            timeout: Duration::from_secs(10),
            check_path: DEFAULT_CHECK_PATH.into(),
            meter_path: DEFAULT_METER_PATH.into(),
        };
        let debug = format!("{cfg:?}");
        assert!(
            !debug.contains(secret),
            "Debug must not contain plaintext api_key: {debug}"
        );
        assert!(
            debug.contains("***"),
            "Debug should show masked api_key: {debug}"
        );

        let key_dbg = format!("{:?}", SecretString::new(secret));
        assert_eq!(key_dbg, "***");
        assert!(!key_dbg.contains(secret));
    }

    #[test]
    fn example_ini_is_canonical_license_section_with_permission_notes() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/license.ini.example");
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("canonical sample missing at {}: {e}", path.display()));
        assert!(
            body.contains("[license]"),
            "example must define [license] section"
        );
        assert!(body.contains("server_url"), "example must document server_url");
        assert!(body.contains("api_key"), "example must document api_key");
        assert!(
            body.contains("timeout_secs"),
            "example must document timeout_secs"
        );
        assert!(body.contains("check_path"), "example must document check_path");
        assert!(body.contains("meter_path"), "example must document meter_path");
        let lower = body.to_lowercase();
        assert!(
            lower.contains("permission")
                || lower.contains("chmod")
                || lower.contains("owner")
                || body.contains("権限")
                || body.contains("読取"),
            "example must document file-permission operational guidance: {body}"
        );
        assert!(
            lower.contains("canonical")
                || lower.contains("正本")
                || body.contains("redefin")
                || body.contains("再定義"),
            "example must state this file is the [license] key canonical source: {body}"
        );
    }
}
