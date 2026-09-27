//! BeatSense Phase-2 ONNX inference reference (Rust / `ort`).
//!
//! Contract mirrors `resources/models/phase2_rev1.onnx.json` and
//! `tools/beatsense/REFERENCE.md`:
//! - input `ecg`: `(B, 10000, 1)` float32 NTC
//! - outputs: `beat`, `event` `[PAC,PVC,N]`, `rhythm` (AF/AFL vs SR)

use crate::inference_options::{BatchSize, CudaTuning};
use crate::model_source::{ModelSource, ModelSourceError};
use ndarray::Array3;
use ort::session::builder::SessionBuilder;
use ort::session::Session;
use ort::value::TensorRef;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};
use thiserror::Error;

mod batch_plan;

pub use batch_plan::{plan_batch, BatchPlan};

pub const MODEL_FS_HZ: u32 = 500;
pub const WINDOW_SEC: f32 = 20.0;
pub const WINDOW_SAMPLES: usize = 10_000;
pub const INPUT_CHANNELS: usize = 1;
pub const INPUT_NAME: &str = "ecg";

pub const EVENT_PAC: usize = 0;
pub const EVENT_PVC: usize = 1;
pub const EVENT_N: usize = 2;

/// Reference thresholds from BeatSense README §5.
pub const TH_BEAT: f32 = 0.90;
pub const TH_PAC: f32 = 0.85;
pub const TH_PVC: f32 = 0.85;
pub const TH_AF: f32 = 0.85;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum BeatClass {
    N,
    Pac,
    Pvc,
}

impl BeatClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::N => "N",
            Self::Pac => "PAC",
            Self::Pvc => "PVC",
        }
    }
}

impl std::fmt::Display for BeatClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RhythmClass {
    #[serde(rename = "SR")]
    Sr,
    #[serde(rename = "AF/AFL")]
    AfAfl,
}

impl RhythmClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Sr => "SR",
            Self::AfAfl => "AF/AFL",
        }
    }

    pub fn from_score(score: f32) -> Self {
        if score >= TH_AF {
            Self::AfAfl
        } else {
            Self::Sr
        }
    }
}

impl std::fmt::Display for RhythmClass {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WindowOutputs {
    /// Sample-level beat probabilities, length [`WINDOW_SAMPLES`].
    pub beat: Vec<f32>,
    /// Sample-level event scores `[PAC, PVC, N]` per sample.
    pub event: Vec<[f32; 3]>,
    /// Window-level rhythm score (high = AF/AFL).
    pub rhythm: f32,
}

impl WindowOutputs {
    pub fn rhythm_class(&self) -> RhythmClass {
        RhythmClass::from_score(self.rhythm)
    }

    /// Rough window-level EVENT summary (mean scores → argmax with thresholds).
    pub fn summary_beat_class(&self) -> BeatClass {
        let mut sum = [0.0_f32; 3];
        for row in &self.event {
            sum[0] += row[0];
            sum[1] += row[1];
            sum[2] += row[2];
        }
        let n = self.event.len().max(1) as f32;
        let mean = [sum[0] / n, sum[1] / n, sum[2] / n];
        let pac_ok = mean[EVENT_PAC] >= TH_PAC;
        let pvc_ok = mean[EVENT_PVC] >= TH_PVC;
        match (pac_ok, pvc_ok) {
            (true, true) => {
                if mean[EVENT_PAC] >= mean[EVENT_PVC] {
                    BeatClass::Pac
                } else {
                    BeatClass::Pvc
                }
            }
            (true, false) => BeatClass::Pac,
            (false, true) => BeatClass::Pvc,
            (false, false) => BeatClass::N,
        }
    }
}

#[derive(Debug, Error)]
pub enum InferError {
    #[error("ONNX model not found: {0}")]
    ModelNotFound(String),
    #[error("model bytes are empty; cannot build inference session")]
    EmptyModelBytes,
    #[error(transparent)]
    ModelSource(#[from] ModelSourceError),
    #[error("invalid ECG window: expected {expected} samples, got {got}")]
    InvalidWindow { expected: usize, got: usize },
    #[error("invalid batch: {count} windows requested, expected 1..={max}")]
    InvalidBatch { count: usize, max: usize },
    #[error("ort error: {0}")]
    Ort(#[from] ort::Error),
    #[error("missing ONNX output: {0}")]
    MissingOutput(&'static str),
    #[error("unexpected output shape for {name}: {detail}")]
    BadShape { name: &'static str, detail: String },
    #[error("execution provider '{0}' is not available on this platform")]
    ProviderUnavailable(String),
}

/// ONNX Runtime execution provider selection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "kebab-case")]
pub enum ExecutionProviderKind {
    /// Prefer CUDA → CPU (compiled / platform availability).
    #[default]
    Auto,
    /// Explicit CPU EP.
    Cpu,
    /// NVIDIA CUDA EP (requires `cuda` feature + CUDA Toolkit / cuDNN at runtime).
    Cuda,
}

impl ExecutionProviderKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Cpu => "cpu",
            Self::Cuda => "cuda",
        }
    }

    /// Providers to try for `auto`, in priority order.
    pub fn auto_candidates() -> &'static [ExecutionProviderKind] {
        &[Self::Cuda, Self::Cpu]
    }
}

impl std::fmt::Display for ExecutionProviderKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl std::str::FromStr for ExecutionProviderKind {
    type Err = String;
    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_ascii_lowercase().as_str() {
            "auto" => Ok(Self::Auto),
            "cpu" => Ok(Self::Cpu),
            "cuda" | "gpu" | "nvidia" => Ok(Self::Cuda),
            other => Err(format!(
                "unknown execution provider '{other}' (expected auto|cpu|cuda)"
            )),
        }
    }
}

/// Batch dimension of the model's `ecg` input.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModelBatchShape {
    /// Symbolic / variable batch dimension.
    Dynamic,
    /// Batch dimension fixed at export time.
    Fixed(usize),
}

/// Requested inference settings: execution provider, batch size, CUDA tuning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct InferenceOptions {
    pub provider: ExecutionProviderKind,
    pub batch_size: BatchSize,
    pub cuda: CudaTuning,
}

impl From<ExecutionProviderKind> for InferenceOptions {
    fn from(provider: ExecutionProviderKind) -> Self {
        Self {
            provider,
            ..Self::default()
        }
    }
}

/// Settings in effect after the session is built.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EffectiveInference {
    /// Resolved provider (never `Auto`).
    pub provider: ExecutionProviderKind,
    /// Windows per inference call.
    pub batch_size: usize,
    pub requested_batch_size: usize,
    pub model_batch: ModelBatchShape,
    /// Short final batches are padded up to `batch_size`.
    pub pad_tail: bool,
    /// Requested CUDA tuning.
    pub cuda: CudaTuning,
    /// True only when the provider is CUDA, i.e. `cuda` was passed to the CUDA EP.
    pub cuda_applied: bool,
    pub cuda_graph_active: bool,
    /// Warnings (fixed-batch clamp, tuning not applied, ...).
    pub notes: Vec<String>,
}

/// Phase-2 ONNX session wrapper.
pub struct Phase2Model {
    session: Session,
    model_path: PathBuf,
    effective: EffectiveInference,
}

impl Phase2Model {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, InferError> {
        Self::load_with_provider(path, ExecutionProviderKind::Auto)
    }

    pub fn load_with_provider(
        path: impl AsRef<Path>,
        provider: ExecutionProviderKind,
    ) -> Result<Self, InferError> {
        Self::load_with_options(path, &InferenceOptions::from(provider))
    }

    pub fn load_with_options(
        path: impl AsRef<Path>,
        options: &InferenceOptions,
    ) -> Result<Self, InferError> {
        // `cargo run --example` places the exe under `target/*/examples/`, while
        // ort's copy-dylibs drops EP DLLs in `target/*/`. Ensure that parent dir
        // is searchable before the CUDA provider library is loaded.
        ensure_ort_dylib_search_path();

        let path = path.as_ref();
        if !path.exists() {
            return Err(InferError::ModelNotFound(path.display().to_string()));
        }

        let path_buf = path.to_path_buf();
        Self::resolve_provider(options.provider, |resolved| {
            Self::commit_from_file_resolved(&path_buf, resolved, options)
        })
    }

    /// Build a session from in-memory model bytes (same EP resolution as path load).
    pub fn load_from_memory(
        model_bytes: &[u8],
        provider: ExecutionProviderKind,
    ) -> Result<Self, InferError> {
        Self::load_from_memory_with(
            model_bytes,
            &InferenceOptions::from(provider),
            PathBuf::from("<memory>"),
        )
    }

    fn load_from_memory_with(
        model_bytes: &[u8],
        options: &InferenceOptions,
        model_path: PathBuf,
    ) -> Result<Self, InferError> {
        ensure_ort_dylib_search_path();
        if model_bytes.is_empty() {
            return Err(InferError::EmptyModelBytes);
        }
        Self::resolve_provider(options.provider, |resolved| {
            Self::commit_from_memory_resolved(model_bytes, resolved, options, model_path.clone())
        })
    }

    /// Resolve [`ModelSource`] to a session: Path → file, Embedded → embedded bytes.
    pub fn load_from_source(
        source: &ModelSource,
        provider: ExecutionProviderKind,
    ) -> Result<Self, InferError> {
        Self::load_from_source_with(source, &InferenceOptions::from(provider))
    }

    /// [`Self::load_from_source`] with batch size and CUDA tuning.
    pub fn load_from_source_with(
        source: &ModelSource,
        options: &InferenceOptions,
    ) -> Result<Self, InferError> {
        source.ensure_supported()?;
        match source {
            ModelSource::Path(path) => Self::load_with_options(path, options),
            ModelSource::Embedded => {
                #[cfg(feature = "embedded-model")]
                {
                    Self::load_from_memory_with(
                        crate::embedded_model::embedded_model_bytes(),
                        options,
                        PathBuf::from("<embedded>"),
                    )
                }
                #[cfg(not(feature = "embedded-model"))]
                {
                    // `ensure_supported` already rejected Embedded without the feature.
                    Err(InferError::ModelSource(
                        ModelSourceError::EmbeddedUnavailable,
                    ))
                }
            }
        }
    }

    /// Shared auto/cpu/cuda fallback used by path and memory load paths.
    fn resolve_provider<F>(provider: ExecutionProviderKind, load: F) -> Result<Self, InferError>
    where
        F: Fn(ExecutionProviderKind) -> Result<Self, InferError>,
    {
        match provider {
            ExecutionProviderKind::Auto => {
                let mut last_err: Option<InferError> = None;
                for candidate in ExecutionProviderKind::auto_candidates() {
                    match load(*candidate) {
                        Ok(model) => return Ok(model),
                        Err(err) => {
                            eprintln!("provider {candidate} unavailable ({err}); trying next ...");
                            last_err = Some(err);
                        }
                    }
                }
                Err(last_err.unwrap_or_else(|| InferError::ProviderUnavailable("auto".into())))
            }
            other => load(other),
        }
    }

    /// CUDA sessions receive the specified tuning values; device 0, FP32 only.
    fn session_builder_for_provider(
        provider: ExecutionProviderKind,
        tuning: &CudaTuning,
    ) -> Result<SessionBuilder, InferError> {
        debug_assert_ne!(provider, ExecutionProviderKind::Auto);

        let builder = Session::builder()?
            .with_no_environment_execution_providers()
            .map_err(ort::Error::<()>::from)?;

        match provider {
            ExecutionProviderKind::Auto => unreachable!("resolved providers only"),
            // Explicit CPU-only session (no env EPs).
            ExecutionProviderKind::Cpu => Ok(builder),
            ExecutionProviderKind::Cuda => {
                #[cfg(feature = "cuda")]
                {
                    // Fail hard so `auto` / explicit `cuda` do not silently run on CPU.
                    let cuda = apply_cuda_tuning(ort::ep::CUDA::default(), tuning)
                        .build()
                        .error_on_failure();
                    Ok(builder
                        .with_execution_providers([cuda])
                        .map_err(ort::Error::<()>::from)?)
                }
                #[cfg(not(feature = "cuda"))]
                {
                    let _ = (builder, tuning);
                    Err(InferError::ProviderUnavailable(
                        "cuda (build with --features cuda)".into(),
                    ))
                }
            }
        }
    }

    fn commit_from_file_resolved(
        path: &Path,
        provider: ExecutionProviderKind,
        options: &InferenceOptions,
    ) -> Result<Self, InferError> {
        let mut builder = Self::session_builder_for_provider(provider, &options.cuda)?;
        let session = builder.commit_from_file(path)?;
        Self::with_effective(session, path.to_path_buf(), provider, options)
    }

    fn commit_from_memory_resolved(
        model_bytes: &[u8],
        provider: ExecutionProviderKind,
        options: &InferenceOptions,
        model_path: PathBuf,
    ) -> Result<Self, InferError> {
        let mut builder = Self::session_builder_for_provider(provider, &options.cuda)?;
        let session = builder.commit_from_memory(model_bytes)?;
        Self::with_effective(session, model_path, provider, options)
    }

    /// Derive the effective settings from the built session and print its warnings.
    fn with_effective(
        session: Session,
        model_path: PathBuf,
        provider: ExecutionProviderKind,
        options: &InferenceOptions,
    ) -> Result<Self, InferError> {
        let model_batch = model_batch_shape(&session)?;
        let is_cuda = provider == ExecutionProviderKind::Cuda;
        let cuda_graph_active = is_cuda && options.cuda.wants_cuda_graph();
        let plan = plan_batch(model_batch, options.batch_size, cuda_graph_active);

        let mut notes = Vec::new();
        if !is_cuda && !options.cuda.is_unset() {
            notes.push(format!(
                "CUDA tuning ({}) not applied: execution provider is {provider}",
                options.cuda.specified_keys().join(", ")
            ));
        }
        notes.extend(plan.note);
        for note in &notes {
            eprintln!("warning: {note}");
        }

        Ok(Self {
            session,
            model_path,
            effective: EffectiveInference {
                provider,
                batch_size: plan.size,
                requested_batch_size: options.batch_size.get(),
                model_batch,
                pad_tail: plan.pad_tail,
                cuda: options.cuda,
                cuda_applied: is_cuda,
                cuda_graph_active,
                notes,
            },
        })
    }

    pub fn model_path(&self) -> &Path {
        &self.model_path
    }

    pub fn provider(&self) -> ExecutionProviderKind {
        self.effective.provider
    }

    pub fn effective(&self) -> &EffectiveInference {
        &self.effective
    }

    /// Effective windows per inference call.
    pub fn batch_size(&self) -> usize {
        self.effective.batch_size
    }

    /// Run inference on one preprocessed window (`WINDOW_SAMPLES` float32 samples).
    pub fn infer_window(&mut self, samples: &[f32]) -> Result<WindowOutputs, InferError> {
        let mut outputs = self.infer_batch(samples, 1)?;
        Ok(outputs.remove(0))
    }

    /// Run `count` consecutive windows (`count * WINDOW_SAMPLES` samples) in one
    /// inference call; returns `count` outputs in input order (padding dropped).
    pub fn infer_batch(
        &mut self,
        windows: &[f32],
        count: usize,
    ) -> Result<Vec<WindowOutputs>, InferError> {
        let max = self.batch_size();
        if count == 0 || count > max {
            return Err(InferError::InvalidBatch { count, max });
        }
        let expected = count * WINDOW_SAMPLES;
        if windows.len() != expected {
            return Err(InferError::InvalidWindow {
                expected,
                got: windows.len(),
            });
        }

        let run_count = if self.effective.pad_tail { max } else { count };
        let input = Array3::from_shape_vec(
            (run_count, WINDOW_SAMPLES, INPUT_CHANNELS),
            padded_input(windows, run_count),
        )
        .expect("shape matches length");

        let outputs = self.session.run(ort::inputs![
            INPUT_NAME => TensorRef::from_array_view(&input)?
        ])?;

        let beat = extract_named_flat(&outputs, "beat", run_count * WINDOW_SAMPLES)?;
        let event = extract_named_flat(&outputs, "event", run_count * WINDOW_SAMPLES * 3)?;
        let rhythm = extract_named_flat(&outputs, "rhythm", run_count)?;
        Ok(split_window_outputs(&beat, &event, &rhythm, count))
    }

    /// One inference over `batch_size()` zero windows; returns its wall time.
    pub fn warm_up(&mut self) -> Result<Duration, InferError> {
        let count = self.batch_size();
        let zeros = vec![0.0_f32; count * WINDOW_SAMPLES];
        let start = Instant::now();
        self.infer_batch(&zeros, count)?;
        Ok(start.elapsed())
    }
}

/// Copy `windows` and zero-fill up to `run_count` windows.
fn padded_input(windows: &[f32], run_count: usize) -> Vec<f32> {
    let mut input = Vec::with_capacity(run_count * WINDOW_SAMPLES);
    input.extend_from_slice(windows);
    input.resize(run_count * WINDOW_SAMPLES, 0.0);
    input
}

/// Slice flat `beat [B,10000,1]`, `event [B,10000,3]`, `rhythm [B,1]` into the
/// first `count` windows.
fn split_window_outputs(
    beat: &[f32],
    event: &[f32],
    rhythm: &[f32],
    count: usize,
) -> Vec<WindowOutputs> {
    beat.chunks_exact(WINDOW_SAMPLES)
        .zip(event.chunks_exact(WINDOW_SAMPLES * 3))
        .zip(rhythm)
        .take(count)
        .map(|((beat, event), &rhythm)| WindowOutputs {
            beat: beat.to_vec(),
            event: event.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
            rhythm,
        })
        .collect()
}

/// Pass only the specified tuning values; `None` keeps the ONNX Runtime default.
#[cfg(feature = "cuda")]
fn apply_cuda_tuning(mut cuda: ort::ep::CUDA, tuning: &CudaTuning) -> ort::ep::CUDA {
    if let Some(enable) = tuning.tf32 {
        cuda = cuda.with_tf32(enable);
    }
    if let Some(enable) = tuning.conv1d_pad_to_nc1d {
        cuda = cuda.with_conv1d_pad_to_nc1d(enable);
    }
    if let Some(enable) = tuning.cuda_graph {
        cuda = cuda.with_cuda_graph(enable);
    }
    cuda
}

/// Batch dimension of the `ecg` input (first input if unnamed); symbolic dims are negative.
fn model_batch_shape(session: &Session) -> Result<ModelBatchShape, InferError> {
    let inputs = session.inputs();
    let input = inputs
        .iter()
        .find(|i| i.name() == INPUT_NAME)
        .or_else(|| inputs.first())
        .ok_or_else(|| InferError::BadShape {
            name: INPUT_NAME,
            detail: "model has no inputs".into(),
        })?;
    let dims = input
        .dtype()
        .tensor_shape()
        .ok_or_else(|| InferError::BadShape {
            name: INPUT_NAME,
            detail: format!("input is not a tensor: {:?}", input.dtype()),
        })?;
    match dims.first() {
        Some(&n) if n > 0 => Ok(ModelBatchShape::Fixed(n as usize)),
        Some(_) => Ok(ModelBatchShape::Dynamic),
        None => Err(InferError::BadShape {
            name: INPUT_NAME,
            detail: "input has no batch dimension".into(),
        }),
    }
}

/// Ensure ort EP shared libraries are loadable when the binary lives under
/// `target/{profile}/examples/` (ort copy-dylibs only fills `target/{profile}/`).
fn ensure_ort_dylib_search_path() {
    let Ok(exe) = std::env::current_exe() else {
        return;
    };
    let Some(exe_dir) = exe.parent() else {
        return;
    };
    let Some(profile_dir) = exe_dir.parent() else {
        return;
    };

    #[cfg(windows)]
    {
        const DYLIBS: &[&str] = &[
            "onnxruntime.dll",
            "onnxruntime_providers_shared.dll",
            "onnxruntime_providers_cuda.dll",
            "onnxruntime_providers_tensorrt.dll",
            "onnxruntime_providers_nv_tensorrt_rtx.dll",
        ];
        for name in DYLIBS {
            let dest = exe_dir.join(name);
            if dest.exists() {
                continue;
            }
            let src = profile_dir.join(name);
            if src.exists() {
                let _ = std::fs::copy(&src, &dest);
            }
        }
    }

    #[cfg(unix)]
    {
        use std::sync::Once;
        static INIT: Once = Once::new();
        INIT.call_once(|| {
            let key = if cfg!(target_os = "macos") {
                "DYLD_LIBRARY_PATH"
            } else {
                "LD_LIBRARY_PATH"
            };
            let mut path = std::env::var_os(key).unwrap_or_default();
            for dir in [exe_dir, profile_dir] {
                let mut entry = dir.as_os_str().to_os_string();
                entry.push(":");
                entry.push(&path);
                path = entry;
            }
            // SAFETY: called once before ORT loads provider libs.
            unsafe { std::env::set_var(key, path) };
        });
    }
}

fn extract_named_flat(
    outputs: &ort::session::SessionOutputs<'_>,
    name: &'static str,
    expected_len: usize,
) -> Result<Vec<f32>, InferError> {
    let value = outputs.get(name).ok_or(InferError::MissingOutput(name))?;
    let (shape, data) = value.try_extract_tensor::<f32>()?;
    let len: usize = shape.iter().map(|d| *d as usize).product();
    if len != expected_len {
        return Err(InferError::BadShape {
            name,
            detail: format!("shape={shape:?}, elems={len}, expected={expected_len}"),
        });
    }
    Ok(data.to_vec())
}

/// Apply the same per-window z-score used in BeatSense external preprocessing.
pub fn zscore_window(samples: &mut [f32]) {
    zscore_window_eps(samples, 1e-6);
}

pub fn zscore_window_eps(samples: &mut [f32], eps: f32) {
    let n = samples.len() as f32;
    if n == 0.0 {
        return;
    }
    let mean = samples.iter().sum::<f32>() / n;
    let var = samples.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / n;
    let std = var.sqrt();
    let std = if std < eps { 1.0 } else { std };
    for x in samples.iter_mut() {
        *x = (*x - mean) / std;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference_options::DEFAULT_BATCH_SIZE;
    use crate::model_source::ModelSource;

    fn smoke_samples() -> Vec<f32> {
        let mut samples = vec![0.0_f32; WINDOW_SAMPLES];
        for (i, s) in samples.iter_mut().enumerate() {
            *s = ((i % 97) as f32) * 0.01 - 0.5;
        }
        zscore_window(&mut samples);
        samples
    }

    #[test]
    fn rhythm_threshold() {
        assert_eq!(RhythmClass::from_score(0.84), RhythmClass::Sr);
        assert_eq!(RhythmClass::from_score(0.85), RhythmClass::AfAfl);
    }

    #[test]
    fn provider_from_str_accepts_auto_cuda() {
        assert_eq!(
            "auto".parse::<ExecutionProviderKind>().unwrap(),
            ExecutionProviderKind::Auto
        );
        assert_eq!(
            "cuda".parse::<ExecutionProviderKind>().unwrap(),
            ExecutionProviderKind::Cuda
        );
        assert_eq!(
            ExecutionProviderKind::auto_candidates(),
            &[ExecutionProviderKind::Cuda, ExecutionProviderKind::Cpu]
        );
    }

    #[test]
    fn load_from_memory_rejects_empty_bytes() {
        let result = Phase2Model::load_from_memory(&[], ExecutionProviderKind::Cpu);
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("empty bytes must fail before inference"),
        };
        assert!(
            matches!(err, InferError::EmptyModelBytes),
            "empty bytes must fail before inference: {err}"
        );
        let msg = err.to_string();
        assert!(
            msg.to_ascii_lowercase().contains("empty")
                || msg.contains("空")
                || msg.contains("byte"),
            "error should clearly mention empty bytes: {msg}"
        );
    }

    #[test]
    fn load_from_source_missing_path_errors_before_infer() {
        let missing = PathBuf::from("/tmp/holter-assist-missing-model-2-2.onnx");
        let source = ModelSource::Path(missing.clone());
        let result = Phase2Model::load_from_source(&source, ExecutionProviderKind::Cpu);
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("missing path must error"),
        };
        assert!(
            matches!(err, InferError::ModelNotFound(_)),
            "missing path must be ModelNotFound: {err}"
        );
        let msg = err.to_string();
        assert!(
            msg.contains(missing.to_string_lossy().as_ref()) || msg.contains("not found"),
            "error should identify the missing path: {msg}"
        );
    }

    #[cfg(not(feature = "embedded-model"))]
    #[test]
    fn load_from_source_embedded_unavailable_without_feature() {
        let result =
            Phase2Model::load_from_source(&ModelSource::Embedded, ExecutionProviderKind::Cpu);
        let err = match result {
            Err(e) => e,
            Ok(_) => panic!("Embedded without feature must fail"),
        };
        let msg = err.to_string();
        assert!(
            msg.contains("embedded-model") || msg.contains("Embedded"),
            "must clearly reject Embedded without feature: {msg}"
        );
    }

    #[test]
    fn load_from_memory_builds_session_with_cpu_ep() {
        let onnx = Path::new("resources/models/phase2_rev1.onnx");
        if !onnx.exists() {
            eprintln!("skip: ONNX not present at {}", onnx.display());
            return;
        }
        let bytes = std::fs::read(onnx).expect("read onnx bytes");
        let mut model =
            Phase2Model::load_from_memory(&bytes, ExecutionProviderKind::Cpu).expect("memory load");
        assert_eq!(model.provider(), ExecutionProviderKind::Cpu);
        let out = model.infer_window(&smoke_samples()).expect("infer");
        assert_eq!(out.beat.len(), WINDOW_SAMPLES);
        assert_eq!(out.event.len(), WINDOW_SAMPLES);
        assert!((0.0..=1.0).contains(&out.rhythm));
    }

    #[test]
    fn path_and_memory_yield_same_output_contract() {
        let onnx = Path::new("resources/models/phase2_rev1.onnx");
        if !onnx.exists() {
            eprintln!("skip: ONNX not present at {}", onnx.display());
            return;
        }
        let bytes = std::fs::read(onnx).expect("read onnx bytes");
        let samples = smoke_samples();

        let mut from_path =
            Phase2Model::load_with_provider(onnx, ExecutionProviderKind::Cpu).expect("path load");
        let mut from_mem =
            Phase2Model::load_from_memory(&bytes, ExecutionProviderKind::Cpu).expect("memory load");

        let path_out = from_path.infer_window(&samples).expect("path infer");
        let mem_out = from_mem.infer_window(&samples).expect("memory infer");

        assert_eq!(path_out.beat.len(), mem_out.beat.len());
        assert_eq!(path_out.event.len(), mem_out.event.len());
        assert_eq!(path_out.beat.len(), WINDOW_SAMPLES);
        assert_eq!(path_out.event.len(), WINDOW_SAMPLES);
        // Same model bytes + same input → same scores (bit-identical float outputs).
        assert_eq!(path_out.rhythm, mem_out.rhythm);
        assert_eq!(path_out.beat, mem_out.beat);
        assert_eq!(path_out.event, mem_out.event);
        assert_eq!(path_out.summary_beat_class(), mem_out.summary_beat_class());
        assert_eq!(path_out.rhythm_class(), mem_out.rhythm_class());
    }

    #[test]
    fn load_from_source_path_matches_load_with_provider() {
        let onnx = Path::new("resources/models/phase2_rev1.onnx");
        if !onnx.exists() {
            eprintln!("skip: ONNX not present at {}", onnx.display());
            return;
        }
        let source = ModelSource::Path(onnx.to_path_buf());
        let mut via_source =
            Phase2Model::load_from_source(&source, ExecutionProviderKind::Cpu).expect("source");
        let mut via_path =
            Phase2Model::load_with_provider(onnx, ExecutionProviderKind::Cpu).expect("path");
        let samples = smoke_samples();
        let a = via_source.infer_window(&samples).expect("source infer");
        let b = via_path.infer_window(&samples).expect("path infer");
        assert_eq!(a.rhythm, b.rhythm);
        assert_eq!(a.beat, b.beat);
        assert_eq!(a.event, b.event);
    }

    #[cfg(feature = "embedded-model")]
    #[test]
    fn load_from_source_embedded_builds_session() {
        let mut model =
            Phase2Model::load_from_source(&ModelSource::Embedded, ExecutionProviderKind::Cpu)
                .expect("embedded load_from_source");
        assert_eq!(model.provider(), ExecutionProviderKind::Cpu);
        assert_eq!(
            model.model_path().to_string_lossy(),
            "<embedded>",
            "Embedded source should use placeholder path label"
        );
        let out = model.infer_window(&smoke_samples()).expect("infer");
        assert_eq!(out.beat.len(), WINDOW_SAMPLES);
        assert_eq!(out.event.len(), WINDOW_SAMPLES);
        assert!((0.0..=1.0).contains(&out.rhythm));
    }

    #[cfg(feature = "embedded-model")]
    #[test]
    fn embedded_and_path_same_contract_when_same_bytes() {
        let onnx = Path::new("resources/models/phase2_rev1.onnx");
        if !onnx.exists() {
            eprintln!("skip: ONNX not present at {}", onnx.display());
            return;
        }
        // When HOLTER_EMBEDDED_MODEL_PATH pointed at the same file used for Path,
        // both routes must yield identical output contract / scores.
        let embedded = crate::embedded_model::embedded_model_bytes();
        let file_bytes = std::fs::read(onnx).expect("read onnx");
        if embedded != file_bytes.as_slice() {
            eprintln!(
                "skip: embedded bytes differ from {}; rebuild with HOLTER_EMBEDDED_MODEL_PATH pointing at it",
                onnx.display()
            );
            return;
        }
        let samples = smoke_samples();
        let mut from_emb =
            Phase2Model::load_from_source(&ModelSource::Embedded, ExecutionProviderKind::Cpu)
                .expect("embedded");
        let mut from_path =
            Phase2Model::load_with_provider(onnx, ExecutionProviderKind::Cpu).expect("path");
        let emb_out = from_emb.infer_window(&samples).expect("emb infer");
        let path_out = from_path.infer_window(&samples).expect("path infer");
        assert_eq!(emb_out.rhythm, path_out.rhythm);
        assert_eq!(emb_out.beat, path_out.beat);
        assert_eq!(emb_out.event, path_out.event);
        assert_eq!(emb_out.summary_beat_class(), path_out.summary_beat_class());
        assert_eq!(emb_out.rhythm_class(), path_out.rhythm_class());
    }

    fn tiny_fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join(name)
    }

    fn cpu_options(batch: usize, cuda: CudaTuning) -> InferenceOptions {
        InferenceOptions {
            provider: ExecutionProviderKind::Cpu,
            batch_size: BatchSize::new(batch).unwrap(),
            cuda,
        }
    }

    #[test]
    fn inference_options_from_provider_uses_defaults() {
        let opts = InferenceOptions::from(ExecutionProviderKind::Cuda);
        assert_eq!(opts.provider, ExecutionProviderKind::Cuda);
        assert_eq!(opts.batch_size, BatchSize::default());
        assert!(opts.cuda.is_unset());
        assert_eq!(
            InferenceOptions::default(),
            InferenceOptions::from(ExecutionProviderKind::Auto)
        );
    }

    #[test]
    fn fixed1_fixture_clamps_requested_batch_with_warning() {
        let opts = cpu_options(16, CudaTuning::default());
        let model = Phase2Model::load_with_options(tiny_fixture("phase2_tiny_fixed1.onnx"), &opts)
            .expect("load fixed1");
        let eff = model.effective();
        assert_eq!(eff.provider, ExecutionProviderKind::Cpu);
        assert_eq!(eff.model_batch, ModelBatchShape::Fixed(1));
        assert_eq!(eff.batch_size, 1);
        assert_eq!(eff.requested_batch_size, 16);
        assert!(!eff.pad_tail);
        assert!(!eff.cuda_applied);
        assert!(!eff.cuda_graph_active);
        assert_eq!(model.batch_size(), 1);
        assert_eq!(eff.notes.len(), 1, "{:?}", eff.notes);
        assert!(eff.notes[0].contains("fixed batch 1"), "{:?}", eff.notes);
        assert!(eff.notes[0].contains("batch_size=16"), "{:?}", eff.notes);
    }

    #[test]
    fn fixed1_fixture_with_matching_request_has_no_notes() {
        let opts = cpu_options(1, CudaTuning::default());
        let model = Phase2Model::load_with_options(tiny_fixture("phase2_tiny_fixed1.onnx"), &opts)
            .expect("load fixed1");
        assert_eq!(model.batch_size(), 1);
        assert!(model.effective().notes.is_empty());
    }

    #[test]
    fn dynamic_fixture_uses_requested_batch() {
        let opts = cpu_options(16, CudaTuning::default());
        let model = Phase2Model::load_with_options(tiny_fixture("phase2_tiny_dynamic.onnx"), &opts)
            .expect("load dynamic");
        let eff = model.effective();
        assert_eq!(
            eff,
            &EffectiveInference {
                provider: ExecutionProviderKind::Cpu,
                batch_size: 16,
                requested_batch_size: 16,
                model_batch: ModelBatchShape::Dynamic,
                pad_tail: false,
                cuda: CudaTuning::default(),
                cuda_applied: false,
                cuda_graph_active: false,
                notes: Vec::new(),
            }
        );
        assert_eq!(model.batch_size(), 16);
    }

    #[test]
    fn cuda_tuning_on_cpu_is_not_applied_and_warned() {
        let tuning = CudaTuning {
            tf32: Some(false),
            conv1d_pad_to_nc1d: Some(true),
            cuda_graph: Some(true),
        };
        let opts = cpu_options(8, tuning);
        let mut model =
            Phase2Model::load_with_options(tiny_fixture("phase2_tiny_dynamic.onnx"), &opts)
                .expect("load dynamic with tuning on cpu");
        let eff = model.effective().clone();
        assert_eq!(eff.provider, ExecutionProviderKind::Cpu);
        assert_eq!(eff.cuda, tuning, "requested tuning is recorded");
        assert!(!eff.cuda_applied);
        assert!(!eff.cuda_graph_active, "cuda graph needs the CUDA provider");
        assert!(!eff.pad_tail, "no fixed shape without an active cuda graph");
        assert_eq!(eff.batch_size, 8);
        assert_eq!(eff.notes.len(), 1, "{:?}", eff.notes);
        let note = &eff.notes[0];
        assert!(note.contains("not applied"), "{note}");
        assert!(note.contains("cpu"), "{note}");
        for key in ["cuda_tf32", "cuda_conv1d_pad_to_nc1d", "cuda_graph"] {
            assert!(note.contains(key), "{note} should name {key}");
        }
        let out = model
            .infer_window(&smoke_samples())
            .expect("inference continues on cpu");
        assert_eq!(out.beat.len(), WINDOW_SAMPLES);
    }

    #[test]
    fn partial_cuda_tuning_warning_names_only_specified_keys() {
        let tuning = CudaTuning {
            tf32: None,
            conv1d_pad_to_nc1d: Some(false),
            cuda_graph: None,
        };
        let model = Phase2Model::load_with_options(
            tiny_fixture("phase2_tiny_dynamic.onnx"),
            &cpu_options(16, tuning),
        )
        .expect("load");
        let notes = &model.effective().notes;
        assert_eq!(notes.len(), 1, "{notes:?}");
        assert!(notes[0].contains("cuda_conv1d_pad_to_nc1d"), "{notes:?}");
        assert!(!notes[0].contains("cuda_tf32"), "{notes:?}");
        assert!(!notes[0].contains("cuda_graph"), "{notes:?}");
    }

    #[test]
    fn fixed1_with_tuning_on_cpu_collects_both_warnings() {
        let tuning = CudaTuning {
            tf32: Some(true),
            ..CudaTuning::default()
        };
        let model = Phase2Model::load_with_options(
            tiny_fixture("phase2_tiny_fixed1.onnx"),
            &cpu_options(16, tuning),
        )
        .expect("load");
        let notes = &model.effective().notes;
        assert_eq!(notes.len(), 2, "{notes:?}");
        assert!(notes.iter().any(|n| n.contains("not applied")), "{notes:?}");
        assert!(
            notes.iter().any(|n| n.contains("fixed batch 1")),
            "{notes:?}"
        );
    }

    #[test]
    fn legacy_load_paths_expose_default_effective_settings() {
        let dynamic = tiny_fixture("phase2_tiny_dynamic.onnx");
        let bytes = std::fs::read(&dynamic).expect("read fixture");
        let via_path = Phase2Model::load_with_provider(&dynamic, ExecutionProviderKind::Cpu)
            .expect("path load");
        let via_memory =
            Phase2Model::load_from_memory(&bytes, ExecutionProviderKind::Cpu).expect("memory");
        let via_source = Phase2Model::load_from_source(
            &ModelSource::Path(dynamic.clone()),
            ExecutionProviderKind::Cpu,
        )
        .expect("source");
        let via_source_with = Phase2Model::load_from_source_with(
            &ModelSource::Path(dynamic.clone()),
            &InferenceOptions::from(ExecutionProviderKind::Cpu),
        )
        .expect("source with options");
        for model in [&via_path, &via_memory, &via_source, &via_source_with] {
            let eff = model.effective();
            assert_eq!(eff.provider, ExecutionProviderKind::Cpu);
            assert_eq!(model.provider(), ExecutionProviderKind::Cpu);
            assert_eq!(eff.model_batch, ModelBatchShape::Dynamic);
            assert_eq!(eff.requested_batch_size, DEFAULT_BATCH_SIZE);
            assert_eq!(eff.batch_size, DEFAULT_BATCH_SIZE);
            assert!(eff.cuda.is_unset());
            assert!(eff.notes.is_empty(), "{:?}", eff.notes);
        }
        assert_eq!(via_memory.model_path(), Path::new("<memory>"));
        assert_eq!(via_source_with.model_path(), dynamic.as_path());
    }

    #[test]
    fn load_with_options_missing_path_is_model_not_found() {
        let missing = PathBuf::from("/tmp/holter-assist-missing-model-inference-options.onnx");
        let err = match Phase2Model::load_with_options(
            &missing,
            &InferenceOptions::from(ExecutionProviderKind::Cpu),
        ) {
            Err(e) => e,
            Ok(_) => panic!("missing path must error"),
        };
        assert!(matches!(err, InferError::ModelNotFound(_)), "{err}");
    }

    #[cfg(feature = "cuda")]
    #[test]
    fn unset_cuda_tuning_passes_nothing_to_cuda_ep() {
        let untouched = format!("{:?}", ort::ep::CUDA::default());
        let applied = format!(
            "{:?}",
            apply_cuda_tuning(ort::ep::CUDA::default(), &CudaTuning::default())
        );
        assert_eq!(applied, untouched);
    }

    #[cfg(feature = "cuda")]
    #[test]
    fn specified_cuda_tuning_sets_matching_ort_options() {
        let tuning = CudaTuning {
            tf32: Some(false),
            conv1d_pad_to_nc1d: Some(true),
            cuda_graph: Some(true),
        };
        let applied = format!("{:?}", apply_cuda_tuning(ort::ep::CUDA::default(), &tuning));
        for (key, value) in [
            ("use_tf32", "0"),
            ("cudnn_conv1d_pad_to_nc1d", "1"),
            ("enable_cuda_graph", "1"),
        ] {
            assert!(
                applied.contains(&format!("{key:?}: {value:?}")),
                "{key}={value} missing in {applied}"
            );
        }
        assert!(!applied.contains("device_id"), "{applied}");

        let only_tf32 = CudaTuning {
            tf32: Some(true),
            ..CudaTuning::default()
        };
        let applied = format!(
            "{:?}",
            apply_cuda_tuning(ort::ep::CUDA::default(), &only_tf32)
        );
        assert!(applied.contains("\"use_tf32\": \"1\""), "{applied}");
        assert!(!applied.contains("cudnn_conv1d_pad_to_nc1d"), "{applied}");
        assert!(!applied.contains("enable_cuda_graph"), "{applied}");
    }

    #[cfg(feature = "embedded-model")]
    #[test]
    fn load_from_source_with_embedded_exposes_effective_settings() {
        let model = Phase2Model::load_from_source_with(
            &ModelSource::Embedded,
            &InferenceOptions::from(ExecutionProviderKind::Cpu),
        )
        .expect("embedded load_from_source_with");
        assert_eq!(model.effective().provider, ExecutionProviderKind::Cpu);
        assert_eq!(model.effective().requested_batch_size, DEFAULT_BATCH_SIZE);
        assert!(model.batch_size() >= 1);
    }

    #[test]
    fn padded_input_zero_fills_to_run_count() {
        let windows: Vec<f32> = (0..2 * WINDOW_SAMPLES).map(|i| i as f32 + 1.0).collect();
        let padded = padded_input(&windows, 4);
        assert_eq!(padded.len(), 4 * WINDOW_SAMPLES);
        assert_eq!(&padded[..2 * WINDOW_SAMPLES], windows.as_slice());
        assert!(padded[2 * WINDOW_SAMPLES..].iter().all(|&v| v == 0.0));
        assert_eq!(padded_input(&windows, 2), windows);
    }

    #[test]
    fn split_window_outputs_slices_contiguously_and_drops_padding() {
        let run_count = 4;
        let beat: Vec<f32> = (0..run_count * WINDOW_SAMPLES).map(|i| i as f32).collect();
        let event: Vec<f32> = (0..run_count * WINDOW_SAMPLES * 3)
            .map(|i| -(i as f32))
            .collect();
        let rhythm = [0.1_f32, 0.2, 0.3, 0.4];
        let out = split_window_outputs(&beat, &event, &rhythm, 3);
        assert_eq!(out.len(), 3);
        for (k, w) in out.iter().enumerate() {
            assert_eq!(
                w.beat.as_slice(),
                &beat[k * WINDOW_SAMPLES..(k + 1) * WINDOW_SAMPLES]
            );
            assert_eq!(w.event.len(), WINDOW_SAMPLES);
            let base = k * WINDOW_SAMPLES * 3;
            assert_eq!(w.event[0], [event[base], event[base + 1], event[base + 2]]);
            assert_eq!(
                w.event[WINDOW_SAMPLES - 1],
                [
                    event[base + 3 * WINDOW_SAMPLES - 3],
                    event[base + 3 * WINDOW_SAMPLES - 2],
                    event[base + 3 * WINDOW_SAMPLES - 1]
                ]
            );
            assert_eq!(w.rhythm, rhythm[k]);
        }
    }

    #[test]
    fn infer_requires_exact_window_len() {
        let onnx = Path::new("resources/models/phase2_rev1.onnx");
        if !onnx.exists() {
            eprintln!("skip: ONNX not present at {}", onnx.display());
            return;
        }
        let mut model = Phase2Model::load(onnx).expect("load onnx");
        let err = model.infer_window(&[0.0; 10]).unwrap_err();
        assert!(matches!(err, InferError::InvalidWindow { .. }));
    }

    #[test]
    fn infer_smoke_random_window() {
        let onnx = Path::new("resources/models/phase2_rev1.onnx");
        if !onnx.exists() {
            eprintln!("skip: ONNX not present at {}", onnx.display());
            return;
        }
        let mut model = Phase2Model::load(onnx).expect("load onnx");
        let out = model.infer_window(&smoke_samples()).expect("infer");
        assert_eq!(out.beat.len(), WINDOW_SAMPLES);
        assert_eq!(out.event.len(), WINDOW_SAMPLES);
        assert!((0.0..=1.0).contains(&out.rhythm));
    }
}
