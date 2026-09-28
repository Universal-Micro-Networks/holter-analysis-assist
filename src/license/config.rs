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
const KEY_LICENSE_KEY: &str = "license_key";
const KEY_TIMEOUT_SECS: &str = "timeout_secs";
const RETIRED_KEY_API_KEY: &str = "api_key";
const RETIRED_ENDPOINT_KEYS: [&str; 2] = ["check_path", "meter_path"];

const DEFAULT_TIMEOUT_SECS: u64 = 10;

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
    /// Base URL, normalized to end with exactly one `/`.
    pub server_url: String,
    pub license_key: SecretString,
    pub timeout: Duration,
}

impl fmt::Debug for LicenseConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("LicenseConfig")
            .field("server_url", &self.server_url)
            .field("license_key", &self.license_key)
            .field("timeout", &self.timeout)
            .finish()
    }
}

/// Ini parse options shared by `[license]` and `[http]` loaders.
///
/// Escapes are disabled so Windows paths such as `C:\Users\...` survive as-is.
pub(crate) fn verbatim_ini_option() -> ini::ParseOption {
    ini::ParseOption {
        enabled_escape: false,
        ..ini::ParseOption::default()
    }
}

impl LicenseConfig {
    /// Load and validate `[license]` settings from `path`.
    ///
    /// Fail-closed: missing file, missing section/keys, invalid URL, non-positive
    /// timeout, or retired keys yield [`LicenseError::Config`].
    pub fn load_from_path(path: &Path) -> Result<Self, LicenseError> {
        let ini = ini::Ini::load_from_file_opt(path, verbatim_ini_option()).map_err(|e| {
            LicenseError::Config(format!(
                "failed to read license ini {}: {e}",
                path.display()
            ))
        })?;

        let section = ini.section(Some(SECTION)).ok_or_else(|| {
            LicenseError::Config(format!("missing [{SECTION}] section in {}", path.display()))
        })?;

        let (config, warning) = Self::from_section(section)?;
        if let Some(warning) = warning {
            eprintln!("license: warning: {warning}");
        }
        Ok(config)
    }

    /// Validate a `[license]` section; the optional string is a non-fatal warning
    /// (never contains key values).
    fn from_section(section: &ini::Properties) -> Result<(Self, Option<String>), LicenseError> {
        for key in RETIRED_ENDPOINT_KEYS {
            if section.contains_key(key) {
                return Err(config_err(format!(
                    "'{key}' in [{SECTION}] is no longer supported: license endpoints are fixed \
                     by the server contract; remove '{key}' (use '{KEY_SERVER_URL}' for a path prefix)"
                )));
            }
        }

        let server_url_raw = section.get(KEY_SERVER_URL).ok_or_else(|| {
            LicenseError::Config(format!(
                "missing required key '{KEY_SERVER_URL}' in [{SECTION}]"
            ))
        })?;
        let server_url = validate_server_url(server_url_raw)?;

        let has_api_key = section.contains_key(RETIRED_KEY_API_KEY);
        let license_key = match section.get(KEY_LICENSE_KEY) {
            Some(v) if !v.trim().is_empty() => SecretString::new(v.trim()),
            Some(_) => {
                return Err(config_err(format!(
                    "'{KEY_LICENSE_KEY}' in [{SECTION}] is empty; set the issued license key"
                )));
            }
            None if has_api_key => {
                return Err(config_err(format!(
                    "'{RETIRED_KEY_API_KEY}' in [{SECTION}] is no longer supported; \
                     rename '{RETIRED_KEY_API_KEY}' to '{KEY_LICENSE_KEY}'"
                )));
            }
            None => {
                return Err(config_err(format!(
                    "missing required key '{KEY_LICENSE_KEY}' in [{SECTION}]; set the issued license key"
                )));
            }
        };
        let warning = has_api_key.then(|| {
            format!(
                "'{RETIRED_KEY_API_KEY}' in [{SECTION}] is ignored because '{KEY_LICENSE_KEY}' \
                 is set; remove '{RETIRED_KEY_API_KEY}'"
            )
        });

        let timeout = match section.get(KEY_TIMEOUT_SECS) {
            Some(raw) => parse_timeout_secs(raw)?,
            None => Duration::from_secs(DEFAULT_TIMEOUT_SECS),
        };

        Ok((
            Self {
                server_url,
                license_key,
                timeout,
            },
            warning,
        ))
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

    let url = reqwest::Url::parse(trimmed)
        .map_err(|e| config_err(format!("invalid '{KEY_SERVER_URL}': not a valid URL ({e})")))?;

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

    Ok(format!("{}/", trimmed.trim_end_matches('/')))
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

#[cfg(test)]
mod tests {
    use super::{
        verbatim_ini_option, LicenseConfig, SecretString, DEFAULT_TIMEOUT_SECS, KEY_LICENSE_KEY,
        KEY_SERVER_URL, KEY_TIMEOUT_SECS, SECTION,
    };
    use crate::license::LicenseError;
    use std::io::Write;
    use std::time::Duration;
    use tempfile::NamedTempFile;

    const KEY: &str = "lk_0123456789abcdef0123456789abcdef";

    fn write_ini(body: &str) -> NamedTempFile {
        let mut f = NamedTempFile::new().expect("temp ini");
        f.write_all(body.as_bytes()).expect("write ini");
        f
    }

    fn load(body: &str) -> Result<LicenseConfig, LicenseError> {
        let ini = write_ini(body);
        LicenseConfig::load_from_path(ini.path())
    }

    fn config_err_message(body: &str) -> String {
        match load(body) {
            Err(LicenseError::Config(msg)) => msg,
            other => panic!("expected LicenseError::Config, got {other:?}"),
        }
    }

    #[test]
    fn loads_required_keys_with_default_timeout() {
        let cfg = load(&format!(
            "[license]\nserver_url=https://license.example.com\nlicense_key={KEY}\n"
        ))
        .expect("valid minimal ini");
        assert_eq!(cfg.server_url, "https://license.example.com/");
        assert_eq!(cfg.license_key.expose_secret(), KEY);
        assert_eq!(cfg.timeout, Duration::from_secs(DEFAULT_TIMEOUT_SECS));
        assert_eq!(DEFAULT_TIMEOUT_SECS, 10);
    }

    #[test]
    fn key_names_are_os_agnostic_literals() {
        assert_eq!(SECTION, "license");
        assert_eq!(KEY_SERVER_URL, "server_url");
        assert_eq!(KEY_LICENSE_KEY, "license_key");
        assert_eq!(KEY_TIMEOUT_SECS, "timeout_secs");
    }

    #[test]
    fn license_key_is_trimmed_and_backslashes_kept_verbatim() {
        let cfg = load("[license]\nserver_url=http://127.0.0.1:9000\nlicense_key=  ab\\tc\\nd  \n")
            .expect("ini with backslashes");
        assert_eq!(cfg.license_key.expose_secret(), r"ab\tc\nd");
    }

    #[test]
    fn license_key_format_is_not_validated_client_side() {
        let cfg =
            load("[license]\nserver_url=https://license.example.com\nlicense_key=not-lk-format\n")
                .expect("any non-empty key is accepted; format is judged by the server");
        assert_eq!(cfg.license_key.expose_secret(), "not-lk-format");
    }

    #[test]
    fn both_keys_present_uses_license_key_and_warns_without_values() {
        let old = "old-api-key-value-must-not-leak";
        let ini = write_ini(&format!(
            "[license]\nserver_url=https://license.example.com\napi_key={old}\nlicense_key={KEY}\n"
        ));
        let parsed = ini::Ini::load_from_file_opt(ini.path(), verbatim_ini_option()).expect("ini");
        let section = parsed.section(Some(SECTION)).expect("section");
        let (cfg, warning) = LicenseConfig::from_section(section).expect("license_key wins");
        assert_eq!(cfg.license_key.expose_secret(), KEY);

        let warning = warning.expect("api_key ignored warning");
        assert!(warning.contains("api_key"), "{warning}");
        assert!(warning.contains("ignored"), "{warning}");
        assert!(
            !warning.contains('\n'),
            "warning must be one line: {warning}"
        );
        assert!(!warning.contains(old), "warning leaked api_key: {warning}");
        assert!(
            !warning.contains(KEY),
            "warning leaked license_key: {warning}"
        );

        let cfg = LicenseConfig::load_from_path(ini.path()).expect("load_from_path");
        assert_eq!(cfg.license_key.expose_secret(), KEY);
    }

    #[test]
    fn license_key_alone_produces_no_warning() {
        let ini = write_ini(&format!(
            "[license]\nserver_url=https://license.example.com\nlicense_key={KEY}\n"
        ));
        let parsed = ini::Ini::load_from_file_opt(ini.path(), verbatim_ini_option()).expect("ini");
        let section = parsed.section(Some(SECTION)).expect("section");
        let (_, warning) = LicenseConfig::from_section(section).expect("ok");
        assert!(warning.is_none(), "{warning:?}");
    }

    #[test]
    fn license_key_is_masked_in_debug() {
        let secret = "lk_plainvalueshouldnotleak0000000000";
        let cfg = LicenseConfig {
            server_url: "https://license.example.com/".into(),
            license_key: SecretString::new(secret),
            timeout: Duration::from_secs(10),
        };
        let debug = format!("{cfg:?}");
        assert!(!debug.contains(secret), "Debug leaked license_key: {debug}");
        assert!(debug.contains("license_key: ***"), "{debug}");

        let key_dbg = format!("{:?}", SecretString::new(secret));
        assert_eq!(key_dbg, "***");
    }

    #[test]
    fn loaded_ini_license_key_is_masked_in_debug() {
        let secret = "ini-loaded-secret-must-not-appear-in-debug";
        let cfg = load(&format!(
            "[license]\nserver_url=https://license.example.com\nlicense_key={secret}\n"
        ))
        .expect("load ini");
        assert_eq!(cfg.license_key.expose_secret(), secret);
        let debug = format!("{cfg:?}");
        assert!(!debug.contains(secret), "Debug leaked license_key: {debug}");
        assert!(debug.contains("***"), "{debug}");
    }

    #[test]
    fn missing_license_key_is_config_error_naming_the_key() {
        let msg = config_err_message("[license]\nserver_url=https://license.example.com\n");
        assert!(msg.contains("license_key"), "{msg}");
    }

    #[test]
    fn empty_license_key_is_config_error_naming_the_key() {
        let msg =
            config_err_message("[license]\nserver_url=https://license.example.com\nlicense_key=\n");
        assert!(msg.contains("license_key"), "{msg}");
    }

    #[test]
    fn whitespace_only_license_key_is_config_error_naming_the_key() {
        let msg = config_err_message(
            "[license]\nserver_url=https://license.example.com\nlicense_key=   \t \n",
        );
        assert!(msg.contains("license_key"), "{msg}");
    }

    #[test]
    fn api_key_only_is_config_error_with_rename_guidance_and_no_value() {
        let secret = "old-api-key-value-must-not-leak";
        let msg = config_err_message(&format!(
            "[license]\nserver_url=https://license.example.com\napi_key={secret}\n"
        ));
        assert!(msg.contains("api_key"), "{msg}");
        assert!(msg.contains("license_key"), "{msg}");
        assert!(msg.to_lowercase().contains("rename"), "{msg}");
        assert!(
            !msg.contains(secret),
            "error must not include key value: {msg}"
        );
    }

    #[test]
    fn check_path_is_rejected_as_no_longer_supported() {
        let msg = config_err_message(&format!(
            "[license]\nserver_url=https://license.example.com\nlicense_key={KEY}\ncheck_path=/v1/license/check\n"
        ));
        assert!(msg.contains("check_path"), "{msg}");
        assert!(msg.contains("no longer supported"), "{msg}");
        assert!(msg.contains("fixed"), "{msg}");
    }

    #[test]
    fn meter_path_is_rejected_as_no_longer_supported() {
        let msg = config_err_message(&format!(
            "[license]\nserver_url=https://license.example.com\nlicense_key={KEY}\nmeter_path=/v1/license/meter\n"
        ));
        assert!(msg.contains("meter_path"), "{msg}");
        assert!(msg.contains("no longer supported"), "{msg}");
        assert!(msg.contains("fixed"), "{msg}");
    }

    #[test]
    fn server_url_without_prefix_gets_one_trailing_slash() {
        let cfg = load(&format!(
            "[license]\nserver_url=http://127.0.0.1:9000\nlicense_key={KEY}\n"
        ))
        .expect("ini");
        assert_eq!(cfg.server_url, "http://127.0.0.1:9000/");
    }

    #[test]
    fn server_url_with_prefix_keeps_prefix_and_gets_trailing_slash() {
        let cfg = load(&format!(
            "[license]\nserver_url=https://host.example.com/prefix\nlicense_key={KEY}\n"
        ))
        .expect("ini");
        assert_eq!(cfg.server_url, "https://host.example.com/prefix/");
    }

    #[test]
    fn server_url_already_ending_with_slash_is_unchanged() {
        let cfg = load(&format!(
            "[license]\nserver_url=https://host.example.com/prefix/\nlicense_key={KEY}\n"
        ))
        .expect("ini");
        assert_eq!(cfg.server_url, "https://host.example.com/prefix/");

        let cfg = load(&format!(
            "[license]\nserver_url=https://host.example.com//\nlicense_key={KEY}\n"
        ))
        .expect("ini");
        assert_eq!(cfg.server_url, "https://host.example.com/");
    }

    #[test]
    fn custom_timeout_is_loaded() {
        let cfg = load(&format!(
            "[license]\nserver_url=https://license.example.com\nlicense_key={KEY}\ntimeout_secs=30\n"
        ))
        .expect("ini");
        assert_eq!(cfg.timeout, Duration::from_secs(30));
    }

    #[test]
    fn missing_server_url_is_config_error_fail_closed() {
        let msg = config_err_message(&format!("[license]\nlicense_key={KEY}\n"));
        assert!(msg.contains("server_url"), "{msg}");
    }

    #[test]
    fn missing_license_section_is_config_error() {
        let err = load(&format!(
            "[other]\nserver_url=https://license.example.com\nlicense_key={KEY}\n"
        ))
        .expect_err("missing section");
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
        let msg = config_err_message(&format!(
            "[license]\nserver_url=not-a-url\nlicense_key={KEY}\n"
        ));
        assert!(msg.contains("server_url"), "{msg}");
    }

    #[test]
    fn empty_server_url_is_config_error() {
        let msg = config_err_message(&format!("[license]\nserver_url=\nlicense_key={KEY}\n"));
        assert!(msg.contains("server_url"), "{msg}");
    }

    #[test]
    fn non_positive_timeout_is_config_error() {
        let msg = config_err_message(&format!(
            "[license]\nserver_url=https://license.example.com\nlicense_key={KEY}\ntimeout_secs=0\n"
        ));
        assert!(msg.contains("timeout_secs"), "{msg}");
    }

    #[test]
    fn invalid_timeout_is_config_error() {
        let msg = config_err_message(&format!(
            "[license]\nserver_url=https://license.example.com\nlicense_key={KEY}\ntimeout_secs=abc\n"
        ));
        assert!(msg.contains("timeout_secs"), "{msg}");
    }

    #[test]
    fn example_ini_is_canonical_license_section_with_permission_notes() {
        let path =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("config/license.ini.example");
        let body = std::fs::read_to_string(&path)
            .unwrap_or_else(|e| panic!("canonical sample missing at {}: {e}", path.display()));
        assert!(
            body.contains("[license]"),
            "example must define [license] section"
        );
        let assigned = |key: &str| {
            body.lines()
                .map(str::trim)
                .any(|l| l.starts_with(&format!("{key}=")))
        };
        assert!(assigned("server_url"), "example must set server_url");
        assert!(
            assigned("license_key"),
            "example must set license_key as a required key: {body}"
        );
        assert!(
            body.contains("timeout_secs"),
            "example must document timeout_secs"
        );
        for retired in ["api_key", "check_path", "meter_path"] {
            assert!(
                !body.contains(retired),
                "example must not document retired key {retired}: {body}"
            );
        }
        assert!(
            body.contains("Authorization"),
            "example must say license_key is sent only in the Authorization header: {body}"
        );
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
