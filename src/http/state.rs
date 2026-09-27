//! Shared HTTP handler state (ModelSource / resident Phase2Model / limits).
//! No analyze LicenseGate ownership.

use crate::http::config::HttpConfig;
use crate::model_source::ModelSource;
use crate::perf::effective_fields;
use crate::phase2::{ExecutionProviderKind, InferError, Phase2Model};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Development default when `[http] model_path` is unset and `embedded-model` is off.
const DEFAULT_DEV_MODEL: &str = "resources/models/phase2_rev1.onnx";

/// How the HTTP process holds the ONNX session.
#[derive(Clone)]
pub enum SharedModel {
    /// Loaded once at startup and reused across requests (production).
    Resident(Arc<Mutex<Phase2Model>>),
    /// Load from [`ModelSource`] on each analyze (unit tests / missing path).
    Ephemeral(ModelSource),
}

/// HTTP handler shared state (design: AppState).
///
/// Holds config / ModelSource / resident model / limits. Must **not** own a
/// per-request `LicenseGate` — metering stays inside analyze via the
/// process-wide global gate.
#[derive(Clone)]
pub struct AppState {
    pub config: HttpConfig,
    pub model_source: ModelSource,
    shared_model: SharedModel,
    warmup: Option<Duration>,
}

impl AppState {
    /// Build state from validated [`HttpConfig`]: load the ONNX session once
    /// with the configured inference options, warm it up, and log readiness.
    ///
    /// Fail-closed: load and warm-up errors propagate to HTTP startup.
    pub fn from_config(config: HttpConfig) -> Result<Self, InferError> {
        let model_source = resolve_model_source(config.model_path.as_ref());
        let mut model =
            Phase2Model::load_from_source_with(&model_source, &config.inference_options())?;
        let warmup = model.warm_up()?;
        eprintln!("{}", ready_line(&model, warmup));
        Ok(Self {
            config,
            model_source,
            shared_model: SharedModel::Resident(Arc::new(Mutex::new(model))),
            warmup: Some(warmup),
        })
    }

    /// Test / lightweight constructor: load the model on each analyze request.
    pub fn new(config: HttpConfig, model_source: ModelSource) -> Self {
        Self {
            config,
            model_source: model_source.clone(),
            shared_model: SharedModel::Ephemeral(model_source),
            warmup: None,
        }
    }

    /// Production-style constructor with an already-loaded session.
    pub fn with_resident(
        config: HttpConfig,
        model_source: ModelSource,
        model: Phase2Model,
    ) -> Self {
        Self {
            config,
            model_source,
            shared_model: SharedModel::Resident(Arc::new(Mutex::new(model))),
            warmup: None,
        }
    }

    pub fn shared_model(&self) -> &SharedModel {
        &self.shared_model
    }

    /// Startup warm-up time; `None` when the model was not warmed up here.
    pub fn warmup_duration(&self) -> Option<Duration> {
        self.warmup
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

/// Readiness log line; load-time `warning:` notes are already printed by
/// [`Phase2Model`] and are not repeated here.
fn ready_line(model: &Phase2Model, warmup: Duration) -> String {
    format!(
        "holter-http-api: model ready {} warmup_ms={:.1} source={}",
        effective_fields(Some(model.effective())),
        warmup.as_secs_f64() * 1000.0,
        model.model_path().display()
    )
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
    use crate::inference_options::{BatchSize, CudaTuning};
    use std::path::PathBuf;
    use std::time::Duration;

    fn minimal_config(model_path: Option<PathBuf>) -> HttpConfig {
        HttpConfig {
            bind: "127.0.0.1:8080".into(),
            max_body_bytes: 1024,
            request_timeout: Duration::from_secs(30),
            model_path,
            provider: ExecutionProviderKind::Cpu,
            batch_size: BatchSize::default(),
            cuda: CudaTuning::default(),
        }
    }

    #[test]
    fn new_uses_ephemeral_model_source() {
        let path = PathBuf::from("/tmp/custom-model.onnx");
        let state = AppState::new(
            minimal_config(Some(path.clone())),
            ModelSource::Path(path.clone()),
        );
        assert_eq!(state.model_source, ModelSource::Path(path));
        assert_eq!(state.max_body_bytes(), 1024);
        assert_eq!(state.provider(), ExecutionProviderKind::Cpu);
        assert!(matches!(state.shared_model(), SharedModel::Ephemeral(_)));
    }

    fn tiny_fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/phase2_tiny_dynamic.onnx")
    }

    #[test]
    fn from_config_loads_with_configured_batch_size_and_warms_up() {
        let mut config = minimal_config(Some(tiny_fixture()));
        config.batch_size = BatchSize::new(4).unwrap();
        let state = AppState::from_config(config).expect("fixture loads and warms up");

        assert!(
            state.warmup_duration().is_some(),
            "resident model must be warmed up before serving"
        );
        let SharedModel::Resident(model) = state.shared_model() else {
            panic!("from_config must hold a resident model");
        };
        let model = model.lock().unwrap();
        assert_eq!(model.batch_size(), 4);
        assert_eq!(model.effective().requested_batch_size, 4);
    }

    #[test]
    fn ready_line_reports_effective_settings_warmup_and_source() {
        let options = crate::phase2::InferenceOptions {
            provider: ExecutionProviderKind::Cpu,
            batch_size: BatchSize::new(4).unwrap(),
            cuda: CudaTuning::default(),
        };
        let model =
            Phase2Model::load_from_source_with(&ModelSource::Path(tiny_fixture()), &options)
                .expect("fixture loads");
        let line = ready_line(&model, Duration::from_micros(12_345));
        assert!(
            line.starts_with(
                "holter-http-api: model ready provider=cpu batch_size=4 \
                 model_batch=dynamic cuda_tuning=not_applied warmup_ms=12.3 source="
            ),
            "{line}"
        );
        assert!(line.contains("phase2_tiny_dynamic.onnx"), "{line}");
    }

    #[cfg(not(feature = "embedded-model"))]
    #[test]
    fn from_config_fails_on_missing_dev_default_path() {
        // Without a real file, startup-style from_config must fail closed.
        let result = AppState::from_config(minimal_config(Some(PathBuf::from(
            "/tmp/http-state-missing.onnx",
        ))));
        assert!(result.is_err(), "missing model must fail");
    }

    #[test]
    fn app_state_does_not_expose_license_gate_field() {
        // Structural invariant (design): no analyze LicenseGate on AppState.
        let state = AppState::new(
            minimal_config(None),
            ModelSource::Path(PathBuf::from(DEFAULT_DEV_MODEL)),
        );
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
