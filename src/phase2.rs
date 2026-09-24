//! BeatSense Phase-2 ONNX inference reference (Rust / `ort`).
//!
//! Contract mirrors `resources/models/phase2_rev1.onnx.json` and
//! `tools/beatsense/REFERENCE.md`:
//! - input `ecg`: `(B, 10000, 1)` float32 NTC
//! - outputs: `beat`, `event` `[PAC,PVC,N]`, `rhythm` (AF/AFL vs SR)

use ndarray::Array3;
use ort::session::Session;
use ort::value::TensorRef;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;

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
    #[error("invalid ECG window: expected {expected} samples, got {got}")]
    InvalidWindow { expected: usize, got: usize },
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

/// Phase-2 ONNX session wrapper.
pub struct Phase2Model {
    session: Session,
    model_path: PathBuf,
    /// Provider that was actually used to build the session (never `Auto`).
    provider: ExecutionProviderKind,
}

impl Phase2Model {
    pub fn load(path: impl AsRef<Path>) -> Result<Self, InferError> {
        Self::load_with_provider(path, ExecutionProviderKind::Auto)
    }

    pub fn load_with_provider(
        path: impl AsRef<Path>,
        provider: ExecutionProviderKind,
    ) -> Result<Self, InferError> {
        // `cargo run --example` places the exe under `target/*/examples/`, while
        // ort's copy-dylibs drops EP DLLs in `target/*/`. Ensure that parent dir
        // is searchable before the CUDA provider library is loaded.
        ensure_ort_dylib_search_path();

        let path = path.as_ref();
        if !path.exists() {
            return Err(InferError::ModelNotFound(path.display().to_string()));
        }

        match provider {
            ExecutionProviderKind::Auto => {
                let mut last_err: Option<InferError> = None;
                for candidate in ExecutionProviderKind::auto_candidates() {
                    match Self::load_resolved(path, *candidate) {
                        Ok(model) => return Ok(model),
                        Err(err) => {
                            eprintln!(
                                "provider {candidate} unavailable ({err}); trying next ..."
                            );
                            last_err = Some(err);
                        }
                    }
                }
                Err(last_err.unwrap_or_else(|| {
                    InferError::ProviderUnavailable("auto".into())
                }))
            }
            other => Self::load_resolved(path, other),
        }
    }

    fn load_resolved(
        path: &Path,
        provider: ExecutionProviderKind,
    ) -> Result<Self, InferError> {
        debug_assert_ne!(provider, ExecutionProviderKind::Auto);

        let mut builder = Session::builder()?
            .with_no_environment_execution_providers()
            .map_err(ort::Error::<()>::from)?;

        match provider {
            ExecutionProviderKind::Auto => unreachable!("resolved providers only"),
            ExecutionProviderKind::Cpu => {
                // Explicit CPU-only session (no env EPs).
            }
            ExecutionProviderKind::Cuda => {
                #[cfg(feature = "cuda")]
                {
                    use ort::ep;

                    // Fail hard so `auto` / explicit `cuda` do not silently run on CPU.
                    let cuda = ep::CUDA::default().build().error_on_failure();
                    builder = builder
                        .with_execution_providers([cuda])
                        .map_err(ort::Error::<()>::from)?;
                }
                #[cfg(not(feature = "cuda"))]
                {
                    return Err(InferError::ProviderUnavailable(
                        "cuda (build with --features cuda)".into(),
                    ));
                }
            }
        }

        let session = builder.commit_from_file(path)?;
        Ok(Self {
            session,
            model_path: path.to_path_buf(),
            provider,
        })
    }

    pub fn model_path(&self) -> &Path {
        &self.model_path
    }

    pub fn provider(&self) -> ExecutionProviderKind {
        self.provider
    }

    /// Run inference on one preprocessed window (`WINDOW_SAMPLES` float32 samples).
    pub fn infer_window(&mut self, samples: &[f32]) -> Result<WindowOutputs, InferError> {
        if samples.len() != WINDOW_SAMPLES {
            return Err(InferError::InvalidWindow {
                expected: WINDOW_SAMPLES,
                got: samples.len(),
            });
        }

        let input = Array3::from_shape_vec((1, WINDOW_SAMPLES, INPUT_CHANNELS), samples.to_vec())
            .expect("shape matches length");

        let outputs = self.session.run(ort::inputs![
            INPUT_NAME => TensorRef::from_array_view(&input)?
        ])?;

        let beat = extract_named_1d(&outputs, "beat", WINDOW_SAMPLES)?;
        let event_flat = extract_named_flat(&outputs, "event", WINDOW_SAMPLES * 3)?;
        let mut event = Vec::with_capacity(WINDOW_SAMPLES);
        for chunk in event_flat.chunks_exact(3) {
            event.push([chunk[0], chunk[1], chunk[2]]);
        }
        let rhythm_vec = extract_named_flat(&outputs, "rhythm", 1)?;
        let rhythm = rhythm_vec[0];

        Ok(WindowOutputs {
            beat,
            event,
            rhythm,
        })
    }
}

fn extract_named_1d(
    outputs: &ort::session::SessionOutputs<'_>,
    name: &'static str,
    expected_len: usize,
) -> Result<Vec<f32>, InferError> {
    let data = extract_named_flat(outputs, name, expected_len)?;
    Ok(data)
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
        let mut samples = vec![0.0_f32; WINDOW_SAMPLES];
        for (i, s) in samples.iter_mut().enumerate() {
            *s = ((i % 97) as f32) * 0.01 - 0.5;
        }
        zscore_window(&mut samples);
        let out = model.infer_window(&samples).expect("infer");
        assert_eq!(out.beat.len(), WINDOW_SAMPLES);
        assert_eq!(out.event.len(), WINDOW_SAMPLES);
        assert!((0.0..=1.0).contains(&out.rhythm));
    }
}
