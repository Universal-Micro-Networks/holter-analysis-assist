//! Shared HTTP handler state (ModelSource / limits). No analyze LicenseGate.

use crate::http::config::HttpConfig;
use crate::model_source::ModelSource;
use crate::phase2::ExecutionProviderKind;
use std::path::PathBuf;
use std::time::Duration;

/// Development default when `[http] model_path` is unset and `embedded-model` is off.
const DEFAULT_DEV_MODEL: &str = "resources/models/phase2_rev1.onnx";

/// HTTP handler shared state (design: AppState).
///
/// Holds config / ModelSource / limits. Must **not** own a per-request
/// `LicenseGate` — metering stays inside `analyze_ecl_with_source` via the
/// process-wide global gate.
#[derive(Debug, Clone)]
pub struct AppState {
    pub config: HttpConfig,
    pub model_source: ModelSource,
}

impl AppState {
    /// Build state from validated [`HttpConfig`], resolving [`ModelSource`]
    /// the same way as CLI: path override, else Embedded (feature), else
    /// development default path.
    pub fn from_config(config: HttpConfig) -> Self {
        let model_source = resolve_model_source(config.model_path.as_ref());
        Self {
            config,
            model_source,
        }
    }

    /// Convenience constructor for tests / callers with explicit source.
    pub fn new(config: HttpConfig, model_source: ModelSource) -> Self {
        Self {
            config,
            model_source,
        }
    }

    pub fn max_body_bytes(&self) -> usize {
        self.config.max_body_bytes
    }

    pub fn request_timeout(&self) -> Duration {
        self.config.request_timeout
    }

    pub fn provider(&self) -> ExecutionProviderKind {
        self.config.provider
    }
}

fn resolve_model_source(model_path: Option<&PathBuf>) -> ModelSource {
    match model_path {
        Some(path) => ModelSource::Path(path.clone()),
        None if cfg!(feature = "embedded-model") => ModelSource::Embedded,
        None => ModelSource::Path(PathBuf::from(DEFAULT_DEV_MODEL)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    fn minimal_config(model_path: Option<PathBuf>) -> HttpConfig {
        HttpConfig {
            bind: "127.0.0.1:8080".into(),
            max_body_bytes: 1024,
            request_timeout: Duration::from_secs(30),
            model_path,
            provider: ExecutionProviderKind::Cpu,
        }
    }

    #[test]
    fn from_config_uses_path_model_source_when_set() {
        let path = PathBuf::from("/tmp/custom-model.onnx");
        let state = AppState::from_config(minimal_config(Some(path.clone())));
        assert_eq!(state.model_source, ModelSource::Path(path));
        assert_eq!(state.max_body_bytes(), 1024);
        assert_eq!(state.provider(), ExecutionProviderKind::Cpu);
    }

    #[cfg(not(feature = "embedded-model"))]
    #[test]
    fn from_config_defaults_to_dev_path_without_embedded_feature() {
        let state = AppState::from_config(minimal_config(None));
        assert_eq!(
            state.model_source,
            ModelSource::Path(PathBuf::from(DEFAULT_DEV_MODEL))
        );
    }

    #[cfg(feature = "embedded-model")]
    #[test]
    fn from_config_defaults_to_embedded_with_feature() {
        let state = AppState::from_config(minimal_config(None));
        assert_eq!(state.model_source, ModelSource::Embedded);
    }

    #[test]
    fn app_state_does_not_expose_license_gate_field() {
        // Structural invariant (design): no analyze LicenseGate on AppState.
        // Compile-time: AppState only has config + model_source.
        let state = AppState::from_config(minimal_config(None));
        let _ = &state.config;
        let _ = &state.model_source;
        let src = include_str!("state.rs");
        assert!(
            !src.lines().any(|l| {
                let t = l.trim();
                (t.starts_with("pub ") || t.starts_with("pub(")) && t.contains("LicenseGate")
            }),
            "AppState must not declare a LicenseGate field (global install only)"
        );
    }
}
