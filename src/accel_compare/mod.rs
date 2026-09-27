//! Accuracy / speed comparison between the baseline inference settings
//! (CPU, FP32, batch size 1, no CUDA tuning) and a candidate setting.
//!
//! Loads the candidate model first (aborting with the reason if its
//! execution provider is unavailable), then the baseline model, analyzes
//! each ECL with both, and collects accuracy differences and per-stage
//! timings into a comparison report. Must not depend on `http`.
//!
//! Every analysis goes through the canonical license gate, so one ECL is
//! metered twice (baseline + candidate).

pub mod metrics;
pub mod report;

use crate::analyze::{analyze_ecl_with_model_observed, AnalyzeError};
use crate::inference_options::{BatchSize, CudaTuning};
use crate::model_source::ModelSource;
use crate::perf::effective_fields;
use crate::phase2::{
    ExecutionProviderKind, InferError, InferenceOptions, Phase2Model, WindowOutputs,
};
use metrics::{MetricsError, WindowAccumulator};
use report::{
    assemble_report, write_report, CompareReport, ConfigInfo, FileObservation, FileReport,
    Thresholds, REPORT_JSON, REPORT_MARKDOWN,
};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::Instant;
use thiserror::Error;

/// 40 samples at 500 Hz = 80 ms.
pub const DEFAULT_TOLERANCE_SAMPLES: u32 = 40;
pub const DEFAULT_PROB_STRIDE: usize = 1;
pub const DEFAULT_REPORT_DIR: &str = "output/accel_compare";
/// Per-ECL analysis CSVs, written under `report_dir/<ECL file stem>/`.
pub const BASELINE_CSV: &str = "baseline.csv";
pub const CANDIDATE_CSV: &str = "candidate.csv";

#[derive(Debug, Clone)]
pub struct CompareConfig {
    /// At least one.
    pub ecl_paths: Vec<PathBuf>,
    /// Used for both the baseline and the candidate.
    pub model: ModelSource,
    pub candidate: InferenceOptions,
    /// Beat matching tolerance in 500 Hz samples.
    pub tolerance_samples: u32,
    /// Sample-level beat / event outputs are compared on every
    /// `prob_stride`-th window (at least 1); rhythm scores on every window.
    pub prob_stride: usize,
    pub max_windows: Option<usize>,
    pub thresholds: Thresholds,
    /// Existing files are overwritten.
    pub report_dir: PathBuf,
}

#[derive(Debug, Error)]
pub enum CompareError {
    /// Rejected before any model is loaded or any ECL analyzed.
    #[error("invalid compare configuration: {0}")]
    Config(String),
    /// The candidate model could not be loaded; nothing was analyzed.
    #[error("candidate provider unavailable: {0}")]
    CandidateUnavailable(InferError),
    #[error(transparent)]
    Analyze(#[from] AnalyzeError),
    #[error(transparent)]
    Infer(#[from] InferError),
    /// A `beat_time` of an analysis row could not be parsed.
    #[error(transparent)]
    Metrics(#[from] MetricsError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// CPU, FP32, batch size 1, no CUDA tuning.
pub fn baseline_options() -> InferenceOptions {
    InferenceOptions {
        provider: ExecutionProviderKind::Cpu,
        batch_size: BatchSize::new(1).expect("1 is a valid batch size"),
        cuda: CudaTuning::default(),
    }
}

/// Analyzes every ECL with the baseline and then the candidate settings,
/// writes the per-ECL CSVs and `report.json` / `report.md` into
/// `report_dir`, and returns the report. Files written before a failure
/// are kept.
pub fn run_compare(cfg: &CompareConfig) -> Result<CompareReport, CompareError> {
    validate(cfg)?;

    let mut candidate_model = load_model("candidate", &cfg.model, &cfg.candidate)
        .map_err(CompareError::CandidateUnavailable)?;
    let baseline = baseline_options();
    let mut baseline_model = load_model("baseline", &cfg.model, &baseline)?;

    eprintln!(
        "compare: {} ECL(s); license is metered twice per ECL (baseline + candidate)",
        cfg.ecl_paths.len()
    );
    let dirs = ecl_output_dirs(&cfg.report_dir, &cfg.ecl_paths);
    let mut files = Vec::with_capacity(cfg.ecl_paths.len());
    for (i, (ecl, dir)) in cfg.ecl_paths.iter().zip(&dirs).enumerate() {
        eprintln!(
            "compare: [{}/{}] {}",
            i + 1,
            cfg.ecl_paths.len(),
            ecl.display()
        );
        files.push(compare_file(
            cfg,
            ecl,
            dir,
            &mut baseline_model,
            &mut candidate_model,
        )?);
    }

    let report = assemble_report(
        ConfigInfo::new(&baseline, Some(baseline_model.effective())),
        ConfigInfo::new(&cfg.candidate, Some(candidate_model.effective())),
        cfg.tolerance_samples,
        files,
        &cfg.thresholds,
    );
    let (json_path, md_path) = write_report(&report, &cfg.report_dir)?;
    eprintln!(
        "compare: wrote {} and {}",
        json_path.display(),
        md_path.display()
    );
    Ok(report)
}

fn validate(cfg: &CompareConfig) -> Result<(), CompareError> {
    if cfg.ecl_paths.is_empty() {
        return Err(CompareError::Config("at least one ECL is required".into()));
    }
    if cfg.prob_stride == 0 {
        return Err(CompareError::Config(
            "prob_stride must be at least 1".into(),
        ));
    }
    Ok(())
}

fn load_model(
    side: &str,
    source: &ModelSource,
    options: &InferenceOptions,
) -> Result<Phase2Model, InferError> {
    let start = Instant::now();
    let model = Phase2Model::load_from_source_with(source, options)?;
    eprintln!(
        "compare: {side} model ready {} load_ms={:.1}",
        effective_fields(Some(model.effective())),
        start.elapsed().as_secs_f64() * 1000.0
    );
    Ok(model)
}

/// Baseline analysis (keeping its window outputs), then the candidate
/// analysis compared window by window as it runs.
fn compare_file(
    cfg: &CompareConfig,
    ecl: &Path,
    dir: &Path,
    baseline: &mut Phase2Model,
    candidate: &mut Phase2Model,
) -> Result<FileReport, CompareError> {
    let mut kept = BaselineWindows::new(cfg.prob_stride);
    let (baseline_rows, baseline_summary) = analyze_ecl_with_model_observed(
        ecl,
        baseline,
        &dir.join(BASELINE_CSV),
        cfg.max_windows,
        &mut |wi: usize, out: &WindowOutputs| kept.record(wi, out),
    )?;
    eprintln!("compare: baseline {}", baseline_summary.perf.log_line());

    let mut windows = WindowAccumulator::default();
    let (candidate_rows, candidate_summary) = analyze_ecl_with_model_observed(
        ecl,
        candidate,
        &dir.join(CANDIDATE_CSV),
        cfg.max_windows,
        &mut |wi: usize, out: &WindowOutputs| kept.compare(wi, out, &mut windows),
    )?;
    eprintln!("compare: candidate {}", candidate_summary.perf.log_line());

    let ecl_label = ecl.display().to_string();
    Ok(FileReport::from_observation(
        &FileObservation {
            ecl: &ecl_label,
            baseline_rows: &baseline_rows,
            candidate_rows: &candidate_rows,
            windows: &windows,
            baseline_perf: &baseline_summary.perf,
            candidate_perf: &candidate_summary.perf,
        },
        cfg.tolerance_samples,
    )?)
}

/// Baseline outputs kept for the candidate run: the rhythm score of every
/// window and the full outputs of every `stride`-th window (window indices
/// arrive in order from 0).
struct BaselineWindows {
    stride: usize,
    rhythm: Vec<f32>,
    sampled: Vec<WindowOutputs>,
}

impl BaselineWindows {
    fn new(stride: usize) -> Self {
        Self {
            stride,
            rhythm: Vec::new(),
            sampled: Vec::new(),
        }
    }

    fn record(&mut self, wi: usize, out: &WindowOutputs) {
        debug_assert_eq!(wi, self.rhythm.len(), "windows arrive in order");
        self.rhythm.push(out.rhythm);
        if wi % self.stride == 0 {
            self.sampled.push(out.clone());
        }
    }

    /// Windows the baseline did not produce are skipped.
    fn compare(&self, wi: usize, out: &WindowOutputs, acc: &mut WindowAccumulator) {
        if let Some(&rhythm) = self.rhythm.get(wi) {
            acc.observe_rhythm(rhythm, out.rhythm);
        }
        if wi % self.stride == 0 {
            if let Some(kept) = self.sampled.get(wi / self.stride) {
                acc.observe_outputs(kept, out);
            }
        }
    }
}

/// `report_dir/<ECL file stem>` per ECL. Repeated stems (compared
/// case-insensitively) get `_2`, `_3`, ... so no ECL overwrites another's
/// CSVs, and the report file names are never used.
fn ecl_output_dirs(report_dir: &Path, ecls: &[PathBuf]) -> Vec<PathBuf> {
    let mut used: HashSet<String> = [REPORT_JSON, REPORT_MARKDOWN]
        .iter()
        .map(|n| n.to_lowercase())
        .collect();
    ecls.iter()
        .map(|ecl| {
            let stem = ecl
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| "ecl".to_string());
            let mut name = stem.clone();
            let mut n = 1;
            while !used.insert(name.to_lowercase()) {
                n += 1;
                name = format!("{stem}_{n}");
            }
            report_dir.join(name)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn outputs(beat: f32, rhythm: f32) -> WindowOutputs {
        WindowOutputs {
            beat: vec![beat],
            event: vec![[beat; 3]],
            rhythm,
        }
    }

    #[test]
    fn baseline_options_are_cpu_batch_one_without_tuning() {
        let b = baseline_options();
        assert_eq!(b.provider, ExecutionProviderKind::Cpu);
        assert_eq!(b.batch_size.get(), 1);
        assert!(b.cuda.is_unset());
    }

    #[test]
    fn baseline_windows_keep_every_rhythm_and_every_stride_th_output() {
        let mut kept = BaselineWindows::new(3);
        for wi in 0..7 {
            kept.record(wi, &outputs(wi as f32 * 0.1, wi as f32 * 0.1));
        }
        assert_eq!(kept.rhythm.len(), 7);
        assert_eq!(kept.sampled.len(), 3, "windows 0, 3, 6");
        assert_eq!(kept.sampled[1].beat, vec![3.0_f32 * 0.1]);

        let mut acc = WindowAccumulator::default();
        for wi in 0..7 {
            let mut out = outputs(wi as f32 * 0.1, wi as f32 * 0.1);
            if wi == 3 {
                out.beat[0] += 0.25;
            }
            if wi == 4 {
                // Not a sampled window: only the rhythm score is compared.
                out.beat[0] += 0.5;
                out.rhythm += 0.125;
            }
            kept.compare(wi, &out, &mut acc);
        }
        assert_eq!(acc.windows(), 7);
        let p = acc.prob_diff();
        assert_eq!(p.sampled_windows, 3);
        assert!((p.beat - 0.25).abs() < 1e-6, "{p:?}");
        assert!((p.rhythm - 0.125).abs() < 1e-6, "{p:?}");
    }

    #[test]
    fn baseline_windows_skip_windows_the_baseline_did_not_produce() {
        let mut kept = BaselineWindows::new(1);
        kept.record(0, &outputs(0.5, 0.5));
        let mut acc = WindowAccumulator::default();
        kept.compare(0, &outputs(0.5, 0.5), &mut acc);
        kept.compare(1, &outputs(0.5, 0.5), &mut acc);
        assert_eq!(acc.windows(), 1);
        assert_eq!(acc.prob_diff().sampled_windows, 1);
    }

    #[test]
    fn ecl_output_dirs_use_stems_and_disambiguate_repeats() {
        let root = Path::new("out");
        let dirs = ecl_output_dirs(
            root,
            &[
                PathBuf::from("a/1_20250101_0000_2359.ecl"),
                PathBuf::from("b/1_20250101_0000_2359.ecl"),
                PathBuf::from("c/1_20250101_0000_2359_2.ecl"),
                PathBuf::from("D.ecl"),
                PathBuf::from("d.ECL"),
                PathBuf::from("report.json.ecl"),
            ],
        );
        assert_eq!(
            dirs,
            vec![
                root.join("1_20250101_0000_2359"),
                root.join("1_20250101_0000_2359_2"),
                root.join("1_20250101_0000_2359_2_2"),
                root.join("D"),
                root.join("d_2"),
                root.join("report.json_2"),
            ]
        );
    }
}
