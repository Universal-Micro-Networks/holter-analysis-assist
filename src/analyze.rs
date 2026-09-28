//! End-to-end ECL analysis pipeline (BeatSense `analyzer.py` Rust port).

use crate::model_source::ModelSource;
use crate::perf::{AnalyzePerf, StageTimings};
use crate::phase2::{
    ExecutionProviderKind, InferenceOptions, Phase2Model, WindowOutputs, WINDOW_SAMPLES,
};
use crate::postprocess::{
    build_rhythm_intervals, build_unknown_intervals, center_best_beats, classify_event_sigmoid,
    cluster_candidates, complement_intervals, detect_run_candidates, extract_window_candidates,
    finalize_runs_after_unknown, label_points_by_rhythm, point_in_intervals, BeatCandidate,
    RhythmWindow,
};
use crate::preprocess::{
    build_ai_continuous_signal, materialize_window, parse_ecl_filename, read_ecl_adc_counts,
    AiContinuousSignal, FS,
};
use chrono::{Duration, NaiveDateTime};
use csv::WriterBuilder;
use serde::Serialize;
use std::path::Path;
use std::time::Instant;
use thiserror::Error;

/// Receives each window's outputs with its job-wide window index, in window order.
pub(crate) type WindowObserver<'a> = &'a mut dyn FnMut(usize, &WindowOutputs);

#[derive(Debug, Error)]
pub enum AnalyzeError {
    #[error(transparent)]
    Preprocess(#[from] crate::preprocess::PreprocessError),
    #[error(transparent)]
    Infer(#[from] crate::phase2::InferError),
    #[error("{0}")]
    Post(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Csv(#[from] csv::Error),
    #[error(transparent)]
    License(#[from] crate::license::LicenseError),
}

#[derive(Debug, Serialize)]
pub struct BeatResultRow {
    pub record_id: String,
    pub beat_idx: i64,
    pub beat_time: String,
    #[serde(rename = "Unknown")]
    pub unknown: i8,
    pub beat_class: String,
    pub rhythm_class: String,
    pub short_run_flag: i8,
}

#[derive(Debug)]
pub struct AnalyzeSummary {
    pub beats: usize,
    pub unknown_ones: usize,
    pub short_run_ones: usize,
    pub windows: usize,
    /// Stage timings and effective inference settings (diagnostics only; not in CSV / JSON).
    pub perf: AnalyzePerf,
}

/// Path-only compatibility wrapper → [`ModelSource::Path`] → [`analyze_ecl_with_source`].
pub fn analyze_ecl(
    ecl_path: &Path,
    onnx_path: &Path,
    output_csv: &Path,
) -> Result<(Vec<BeatResultRow>, AnalyzeSummary), AnalyzeError> {
    analyze_ecl_with_source(
        ecl_path,
        &ModelSource::Path(onnx_path.to_path_buf()),
        output_csv,
        None,
        &InferenceOptions::from(ExecutionProviderKind::Auto),
    )
}

/// Path-only compatibility wrapper → [`ModelSource::Path`] → [`analyze_ecl_with_source`].
pub fn analyze_ecl_with_limit(
    ecl_path: &Path,
    onnx_path: &Path,
    output_csv: &Path,
    max_windows: Option<usize>,
    provider: ExecutionProviderKind,
) -> Result<(Vec<BeatResultRow>, AnalyzeSummary), AnalyzeError> {
    analyze_ecl_with_source(
        ecl_path,
        &ModelSource::Path(onnx_path.to_path_buf()),
        output_csv,
        max_windows,
        &InferenceOptions::from(provider),
    )
}

/// Canonical ECL analysis entry: load the model via [`ModelSource`] with `options`
/// (provider, batch size, CUDA tuning), then run the pipeline.
///
/// License authorize+meter runs once per job via the process-wide [`crate::license::LicenseGate`],
/// after model load and preprocess and before the first window inference.
pub fn analyze_ecl_with_source(
    ecl_path: &Path,
    model: &ModelSource,
    output_csv: &Path,
    max_windows: Option<usize>,
    options: &InferenceOptions,
) -> Result<(Vec<BeatResultRow>, AnalyzeSummary), AnalyzeError> {
    let job_start = Instant::now();
    // Fail-fast on model source before reading the full ECL (Path missing / Embedded unavailable).
    let load_start = Instant::now();
    let mut model = Phase2Model::load_from_source_with(model, options)?;
    let model_load = load_start.elapsed();
    analyze_ecl_with_loaded_model(
        ecl_path,
        &mut model,
        output_csv,
        max_windows,
        &mut |_, _| {},
        job_start,
        Some(model_load),
    )
}

/// Run the ECL pipeline with an already-loaded [`Phase2Model`] (HTTP resident session).
/// Batch size and tuning follow the model's effective settings.
///
/// License authorize+meter runs once per job via the process-wide [`crate::license::LicenseGate`],
/// after preprocess and before the first window inference.
pub fn analyze_ecl_with_model(
    ecl_path: &Path,
    model: &mut Phase2Model,
    output_csv: &Path,
    max_windows: Option<usize>,
) -> Result<(Vec<BeatResultRow>, AnalyzeSummary), AnalyzeError> {
    analyze_ecl_with_model_observed(ecl_path, model, output_csv, max_windows, &mut |_, _| {})
}

/// [`analyze_ecl_with_model`] that also hands every window's outputs to `observer`.
///
/// License authorize+meter runs once per job via the process-wide [`crate::license::LicenseGate`],
/// after preprocess and before the first window inference.
pub(crate) fn analyze_ecl_with_model_observed(
    ecl_path: &Path,
    model: &mut Phase2Model,
    output_csv: &Path,
    max_windows: Option<usize>,
    observer: WindowObserver<'_>,
) -> Result<(Vec<BeatResultRow>, AnalyzeSummary), AnalyzeError> {
    let job_start = Instant::now();
    analyze_ecl_with_loaded_model(
        ecl_path,
        model,
        output_csv,
        max_windows,
        observer,
        job_start,
        None,
    )
}

fn ensure_inference_licensed() -> Result<(), AnalyzeError> {
    // Fail-closed: uninstalled gate or meter deny rejects before any analyze output.
    match crate::license::LicenseGate::try_global() {
        None => Err(crate::license::LicenseError::InferenceDenied(
            crate::license::LicenseFailure::new(
                crate::license::LicenseFailureReason::GateNotInstalled,
                "license gate not installed",
            ),
        )
        .into()),
        Some(gate) => {
            gate.ensure_inference_allowed()?;
            Ok(())
        }
    }
}

/// Candidates and rhythm windows of all inferred windows, in window order.
struct WindowInference {
    candidates: Vec<BeatCandidate>,
    rhythm_windows: Vec<RhythmWindow>,
}

/// Infer `starts` in chunks of the model's effective batch size.
///
/// Window indices stay job-wide (not per chunk), so candidates and rhythm
/// windows come out identical for every batch size.
fn infer_windows(
    model: &mut Phase2Model,
    signal: &AiContinuousSignal,
    starts: &[i64],
    observer: WindowObserver<'_>,
) -> Result<WindowInference, AnalyzeError> {
    let batch = model.batch_size();
    let mut candidates = Vec::new();
    let mut rhythm_windows = Vec::with_capacity(starts.len());
    let mut flat = Vec::with_capacity(batch * WINDOW_SAMPLES);

    for (chunk_index, chunk) in starts.chunks(batch).enumerate() {
        flat.clear();
        for &start_abs in chunk {
            flat.extend_from_slice(&materialize_window(
                &signal.ecg_valid_500,
                signal.abs_valid_start_500,
                start_abs,
            ));
        }
        let outputs = model.infer_batch(&flat, chunk.len())?;
        for (offset, (&start_abs, out)) in chunk.iter().zip(&outputs).enumerate() {
            let wi = chunk_index * batch + offset;
            observer(wi, out);
            candidates.extend(extract_window_candidates(
                &out.beat, &out.event, start_abs, wi,
            ));
            rhythm_windows.push(RhythmWindow {
                start_sample_500: start_abs,
                end_sample_500: start_abs + WINDOW_SAMPLES as i64,
                rhythm_score: out.rhythm,
            });
            if wi > 0 && wi.is_multiple_of(200) {
                eprintln!("      ... window {wi}/{}", starts.len());
            }
        }
    }

    Ok(WindowInference {
        candidates,
        rhythm_windows,
    })
}

fn analyze_ecl_with_loaded_model(
    ecl_path: &Path,
    model: &mut Phase2Model,
    output_csv: &Path,
    max_windows: Option<usize>,
    observer: WindowObserver<'_>,
    job_start: Instant,
    model_load: Option<std::time::Duration>,
) -> Result<(Vec<BeatResultRow>, AnalyzeSummary), AnalyzeError> {
    let preprocess_start = Instant::now();
    let source_info = parse_ecl_filename(ecl_path)?;
    let ecl_name = ecl_path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("<ecl>");

    let ecg_all_250 = read_ecl_adc_counts(ecl_path)?;
    eprintln!("[1/6] AI preprocessing ...");
    let signal = build_ai_continuous_signal(&ecg_all_250, &source_info)?;
    let preprocess = preprocess_start.elapsed();

    let mut starts = signal.starts_abs_500.clone();
    if let Some(limit) = max_windows {
        starts.truncate(limit);
    }

    // Window count follows the filename recording range (not the full ECL buffer),
    // capped at MAX_RECORDING_DAYS (7). Example: 14:15–23:59 → ~2064 windows.
    eprintln!(
        "[1/6] ECL {} recording {} .. {} -> valid_500={} starts={} (step={}s)",
        ecl_name,
        source_info.recording_start,
        source_info.recording_end,
        signal.ecg_valid_500.len(),
        signal.starts_abs_500.len(),
        crate::preprocess::STEP_SEC
    );

    // Invalid input or model must fail before this point so it consumes no usage.
    ensure_inference_licensed()?;

    eprintln!(
        "[2/6] ONNX inference: windows={} using_provider={} (model resident={})",
        starts.len(),
        model.provider(),
        model.model_path().display()
    );
    let inference_start = Instant::now();
    let WindowInference {
        candidates: all_candidates,
        rhythm_windows,
    } = infer_windows(model, &signal, &starts, observer)?;
    let inference = inference_start.elapsed();

    eprintln!("[3/6] Beat / event / rhythm post-processing ...");
    let postprocess_start = Instant::now();
    let beats = center_best_beats(cluster_candidates(all_candidates));
    let (rhythm_intervals, coverage_intervals) = build_rhythm_intervals(rhythm_windows);
    let beat_positions: Vec<i64> = beats.iter().map(|b| b.sample_in_file_500).collect();
    let rhythm_labels = label_points_by_rhythm(&beat_positions, &rhythm_intervals);

    let raw_classes: Vec<&str> = beats
        .iter()
        .map(|b| classify_event_sigmoid(b.pac_score, b.pvc_score))
        .collect();
    let mut final_classes: Vec<String> = raw_classes.iter().map(|s| (*s).to_string()).collect();
    for (i, label) in rhythm_labels.iter().enumerate() {
        if raw_classes[i] == "PAC" && label == "AF/AFL" {
            final_classes[i] = "N".to_string(); // PAC_MASKED_AF → N for CSV
        }
    }

    let af_intervals: Vec<(i64, i64)> = rhythm_intervals
        .iter()
        .filter(|r| r.label == "AF/AFL")
        .map(|r| (r.start, r.end))
        .collect();
    let gap_intervals = complement_intervals(
        signal.abs_valid_start_500,
        signal.abs_valid_end_500,
        &coverage_intervals,
    );

    eprintln!("[4/6] RRI RUN candidates ...");
    let class_for_run: Vec<String> = final_classes.clone();
    let (run_candidates, _pre_unknown_members) =
        detect_run_candidates(&beats, &class_for_run, &af_intervals, &gap_intervals);

    eprintln!("[5/6] Unknown frequency QC ...");
    let (unknown_intervals, _median, _thr) = build_unknown_intervals(
        &ecg_all_250,
        &source_info,
        signal.abs_valid_start_500,
        signal.abs_valid_end_500,
    )
    .map_err(AnalyzeError::Post)?;

    let beat_in_coverage = point_in_intervals(&beat_positions, &coverage_intervals);
    let unknown_hits = point_in_intervals(&beat_positions, &unknown_intervals);
    let unknown: Vec<i8> = beat_in_coverage
        .iter()
        .zip(unknown_hits.iter())
        .map(|(c, u)| if *c && *u { 1 } else { 0 })
        .collect();

    let accepted_run_members = finalize_runs_after_unknown(run_candidates, &unknown_intervals);
    let short_run_flag: Vec<i8> = beat_positions
        .iter()
        .map(|p| {
            if accepted_run_members.contains(p) {
                1
            } else {
                0
            }
        })
        .collect();

    // Unknown beats cannot keep short_run_flag=1
    let short_run_flag: Vec<i8> = short_run_flag
        .iter()
        .zip(unknown.iter())
        .map(|(s, u)| if *u == 1 { 0 } else { *s })
        .collect();
    let postprocess = postprocess_start.elapsed();

    eprintln!("[6/6] Final CSV ...");
    let output_start = Instant::now();
    let record_id = ecl_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string();
    let file_start = source_info.study_date.and_hms_opt(0, 0, 0).unwrap();

    let mut rows = Vec::with_capacity(beats.len());
    for (i, beat) in beats.iter().enumerate() {
        let t = sample_to_time(file_start, beat.sample_in_file_500);
        rows.push(BeatResultRow {
            record_id: record_id.clone(),
            beat_idx: i as i64,
            beat_time: format_beat_time(t),
            unknown: unknown[i],
            beat_class: final_classes[i].clone(),
            rhythm_class: if rhythm_labels[i].is_empty() {
                "UNCOVERED".into()
            } else {
                rhythm_labels[i].clone()
            },
            short_run_flag: short_run_flag[i],
        });
    }

    if let Some(parent) = output_csv.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut wtr = WriterBuilder::new()
        .has_headers(true)
        .from_path(output_csv)?;
    for row in &rows {
        wtr.serialize(row)?;
    }
    wtr.flush()?;

    // Integrity checks (BeatSense analyzer)
    for row in &rows {
        if row.unknown == 1 && row.short_run_flag == 1 {
            return Err(AnalyzeError::Post(
                "Unknown beat cannot have short_run_flag=1".into(),
            ));
        }
        if row.rhythm_class == "AF/AFL" && row.beat_class == "PAC" {
            return Err(AnalyzeError::Post(
                "PAC must be masked inside AF/AFL".into(),
            ));
        }
    }

    let output = output_start.elapsed();

    let summary = AnalyzeSummary {
        beats: rows.len(),
        unknown_ones: rows.iter().filter(|r| r.unknown == 1).count(),
        short_run_ones: rows.iter().filter(|r| r.short_run_flag == 1).count(),
        windows: starts.len(),
        perf: AnalyzePerf {
            timings: StageTimings {
                model_load,
                preprocess,
                inference,
                postprocess,
                output,
                total: job_start.elapsed(),
            },
            windows: starts.len(),
            effective: Some(model.effective().clone()),
        },
    };
    Ok((rows, summary))
}

fn sample_to_time(file_start: NaiveDateTime, sample_500: i64) -> NaiveDateTime {
    let ms = (sample_500 as f64 / FS as f64 * 1000.0).round() as i64;
    file_start + Duration::milliseconds(ms)
}

fn format_beat_time(t: NaiveDateTime) -> String {
    t.format("%Y-%m-%d %H:%M:%S%.3f").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference_options::{BatchSize, CudaTuning};
    use crate::license::{
        LicenseCheckResult, LicenseClient, LicenseError, LicenseFailureReason, LicenseGate,
        LicenseMeterResult, MockLicenseClient, MockOutcome, GLOBAL_TEST_LOCK,
    };
    use crate::model_source::ModelSource;
    use crate::phase2::InferError;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::time::Duration;
    use tempfile::TempDir;

    /// Valid filename shape so preprocess filename parse succeeds; file need not exist
    /// when model load fails first via `ModelSource`.
    fn stub_ecl_path() -> PathBuf {
        PathBuf::from("1234567890_20250101_0000_2359.ecl")
    }

    fn temp_csv() -> (TempDir, PathBuf) {
        let dir = TempDir::new().expect("tempdir");
        let csv = dir.path().join("out.csv");
        (dir, csv)
    }

    fn with_clean_global(f: impl FnOnce()) {
        let _guard = GLOBAL_TEST_LOCK.lock().unwrap_or_else(|e| e.into_inner());
        LicenseGate::clear_for_test();
        f();
        LicenseGate::clear_for_test();
    }

    fn install_allow_gate() {
        LicenseGate::install(LicenseGate::new(MockLicenseClient::new(
            MockOutcome::Success { message: None },
            MockOutcome::Success { message: None },
        )))
        .expect("install allow gate");
    }

    /// Counts meter calls to prove single authorize_and_meter at the canonical entry.
    struct CountingMeterClient {
        inner: MockLicenseClient,
        meter_calls: Arc<AtomicUsize>,
    }

    impl CountingMeterClient {
        fn allow(meter_calls: Arc<AtomicUsize>) -> Self {
            Self {
                inner: MockLicenseClient::new(
                    MockOutcome::Success { message: None },
                    MockOutcome::Success { message: None },
                ),
                meter_calls,
            }
        }
    }

    impl LicenseClient for CountingMeterClient {
        fn check_validity(&self) -> Result<LicenseCheckResult, LicenseError> {
            self.inner.check_validity()
        }

        fn authorize_and_meter(&self) -> Result<LicenseMeterResult, LicenseError> {
            self.meter_calls.fetch_add(1, Ordering::SeqCst);
            self.inner.authorize_and_meter()
        }
    }

    fn install_counting_gate() -> Arc<AtomicUsize> {
        let meter_calls = Arc::new(AtomicUsize::new(0));
        LicenseGate::install(LicenseGate::new(CountingMeterClient::allow(Arc::clone(
            &meter_calls,
        ))))
        .expect("install counting gate");
        meter_calls
    }

    #[test]
    fn analyze_ecl_with_source_missing_path_errors_before_ecl_read() {
        with_clean_global(|| {
            let meter_calls = install_counting_gate();
            let (_dir, csv) = temp_csv();
            let missing = PathBuf::from("/tmp/holter-assist-missing-model-3-1.onnx");
            let source = ModelSource::Path(missing.clone());
            let err = analyze_ecl_with_source(
                &stub_ecl_path(),
                &source,
                &csv,
                Some(0),
                &InferenceOptions::from(ExecutionProviderKind::Cpu),
            )
            .expect_err("missing model path must fail");
            match err {
                AnalyzeError::Infer(InferError::ModelNotFound(p)) => {
                    assert!(
                        p.contains(missing.to_string_lossy().as_ref()) || p.contains("not found"),
                        "error should identify missing path: {p}"
                    );
                }
                other => panic!("expected Infer(ModelNotFound), got {other}"),
            }
            assert!(!csv.exists(), "must not write CSV when model load fails");
            assert_eq!(
                meter_calls.load(Ordering::SeqCst),
                0,
                "model load failure must not consume usage"
            );
        });
    }

    #[test]
    fn analyze_ecl_with_limit_wrapper_delegates_path_source() {
        with_clean_global(|| {
            let meter_calls = install_counting_gate();
            let (_dir, csv) = temp_csv();
            let missing = PathBuf::from("/tmp/holter-assist-missing-model-3-1-wrap.onnx");
            let err = analyze_ecl_with_limit(
                &stub_ecl_path(),
                &missing,
                &csv,
                Some(0),
                ExecutionProviderKind::Cpu,
            )
            .expect_err("wrapper must surface missing Path via ModelSource");
            assert!(
                matches!(err, AnalyzeError::Infer(InferError::ModelNotFound(_))),
                "wrapper should load via ModelSource::Path: {err}"
            );
            assert_eq!(meter_calls.load(Ordering::SeqCst), 0);
        });
    }

    #[cfg(not(feature = "embedded-model"))]
    #[test]
    fn analyze_ecl_with_source_embedded_rejected_without_feature() {
        with_clean_global(|| {
            install_allow_gate();
            let (_dir, csv) = temp_csv();
            let err = analyze_ecl_with_source(
                &stub_ecl_path(),
                &ModelSource::Embedded,
                &csv,
                Some(0),
                &InferenceOptions::from(ExecutionProviderKind::Cpu),
            )
            .expect_err("Embedded without feature must fail");
            let msg = err.to_string();
            assert!(
                msg.contains("embedded-model") || msg.contains("Embedded"),
                "must clearly reject Embedded without feature: {msg}"
            );
            assert!(!csv.exists());
        });
    }

    #[test]
    fn analyze_ecl_with_source_path_smoke_optional() {
        with_clean_global(|| {
            install_allow_gate();
            let sample_link = Path::new("resources/samples/sample.ecl");
            let onnx = Path::new("resources/models/phase2_rev1.onnx");
            if !sample_link.exists() || !onnx.exists() {
                eprintln!("skip: sample.ecl or ONNX not present");
                return;
            }
            // Symlink basename is `sample.ecl`; resolve to the real file whose name matches ECL rules.
            let sample = match sample_link.canonicalize() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("skip: cannot canonicalize sample.ecl: {e}");
                    return;
                }
            };
            let (_dir, csv) = temp_csv();
            let source = ModelSource::Path(onnx.to_path_buf());
            let (rows, summary) = analyze_ecl_with_source(
                &sample,
                &source,
                &csv,
                Some(1),
                &InferenceOptions::from(ExecutionProviderKind::Cpu),
            )
            .expect("path source analyze with 1 window");
            assert_eq!(summary.windows, 1);
            assert!(csv.exists());
            assert_eq!(rows.len(), summary.beats);
        });
    }

    #[cfg(feature = "embedded-model")]
    #[test]
    fn analyze_ecl_with_source_embedded_smoke_optional() {
        with_clean_global(|| {
            install_allow_gate();
            let sample_link = Path::new("resources/samples/sample.ecl");
            if !sample_link.exists() {
                eprintln!("skip: sample.ecl not present");
                return;
            }
            let sample = match sample_link.canonicalize() {
                Ok(p) => p,
                Err(e) => {
                    eprintln!("skip: cannot canonicalize sample.ecl: {e}");
                    return;
                }
            };
            let (_dir, csv) = temp_csv();
            let (rows, summary) = analyze_ecl_with_source(
                &sample,
                &ModelSource::Embedded,
                &csv,
                Some(1),
                &InferenceOptions::from(ExecutionProviderKind::Cpu),
            )
            .expect("embedded source analyze without external model path");
            assert_eq!(summary.windows, 1);
            assert!(csv.exists());
            assert_eq!(rows.len(), summary.beats);
        });
    }

    // --- License gate at canonical analyze entry (task 4.1) ---

    #[test]
    fn analyze_ecl_with_source_uninstalled_gate_fail_closed_no_output() {
        with_clean_global(|| {
            let dir = TempDir::new().expect("tempdir");
            let ecl = write_synthetic_ecl(dir.path());
            let csv = dir.path().join("out.csv");
            let err = analyze_ecl_with_source(
                &ecl,
                &ModelSource::Path(tiny_dynamic_fixture()),
                &csv,
                Some(2),
                &cpu_options(4),
            )
            .expect_err("uninstalled gate must fail-closed");
            let msg = err.to_string();
            assert!(
                msg.contains("inference denied"),
                "must surface inference denial: {msg}"
            );
            assert!(
                matches!(
                    &err,
                    AnalyzeError::License(LicenseError::InferenceDenied(f))
                        if f.reason == LicenseFailureReason::GateNotInstalled
                ),
                "expected License(InferenceDenied(gate_not_installed)), got {err:?}"
            );
            assert!(
                msg.contains("gate_not_installed"),
                "must surface the reason code: {msg}"
            );
            assert!(!csv.exists(), "must not write CSV when gate missing");
        });
    }

    #[test]
    fn analyze_ecl_with_source_meter_deny_no_output_or_rows() {
        with_clean_global(|| {
            LicenseGate::install(LicenseGate::new(MockLicenseClient::new(
                MockOutcome::Success { message: None },
                MockOutcome::Reject {
                    reason: LicenseFailureReason::MonthlyLimitReached,
                    message: Some("quota exceeded".into()),
                },
            )))
            .expect("install deny gate");

            let dir = TempDir::new().expect("tempdir");
            let ecl = write_synthetic_ecl(dir.path());
            let csv = dir.path().join("out.csv");
            let err = analyze_ecl_with_source(
                &ecl,
                &ModelSource::Path(tiny_dynamic_fixture()),
                &csv,
                Some(2),
                &cpu_options(4),
            )
            .expect_err("meter deny must reject inference");
            let msg = err.to_string();
            assert!(
                msg.contains("inference denied") && msg.contains("quota exceeded"),
                "must identify inference denial: {msg}"
            );
            assert!(
                matches!(
                    &err,
                    AnalyzeError::License(LicenseError::InferenceDenied(f))
                        if f.reason == LicenseFailureReason::MonthlyLimitReached
                ),
                "expected License(InferenceDenied(monthly_limit_reached)), got {err:?}"
            );
            assert!(!csv.exists(), "must not write CSV on meter deny");

            let mut model = load_tiny(4);
            let mut calls = 0usize;
            let err = analyze_ecl_with_model_observed(
                &ecl,
                &mut model,
                &csv,
                Some(2),
                &mut |_wi: usize, _out: &WindowOutputs| calls += 1,
            )
            .expect_err("meter deny must reject resident-model inference");
            assert!(
                matches!(
                    &err,
                    AnalyzeError::License(LicenseError::InferenceDenied(f))
                        if f.reason == LicenseFailureReason::MonthlyLimitReached
                ),
                "got {err:?}"
            );
            assert_eq!(calls, 0, "no window may be inferred after meter deny");
            assert!(!csv.exists(), "must not write CSV on meter deny");
        });
    }

    #[test]
    fn analyze_ecl_with_source_meters_once_even_with_multiple_windows() {
        with_clean_global(|| {
            let meter_calls = install_counting_gate();
            let dir = TempDir::new().expect("tempdir");
            let ecl = write_synthetic_ecl(dir.path());
            let csv = dir.path().join("out.csv");
            let (rows, summary) = analyze_ecl_with_source(
                &ecl,
                &ModelSource::Path(tiny_dynamic_fixture()),
                &csv,
                Some(3),
                &cpu_options(1),
            )
            .expect("meter success should allow analyze");
            assert_eq!(summary.windows, 3, "exercise multiple windows");
            assert!(csv.exists());
            assert!(!rows.is_empty() || summary.beats == 0);
            assert_eq!(
                meter_calls.load(Ordering::SeqCst),
                1,
                "multiple windows must not re-meter"
            );
        });
    }

    #[test]
    fn analyze_ecl_wrappers_do_not_double_meter() {
        with_clean_global(|| {
            let meter_calls = install_counting_gate();
            let dir = TempDir::new().expect("tempdir");
            let ecl = write_synthetic_ecl(dir.path());

            let csv = dir.path().join("limit.csv");
            let (_rows, summary) = analyze_ecl_with_limit(
                &ecl,
                &tiny_dynamic_fixture(),
                &csv,
                Some(2),
                ExecutionProviderKind::Cpu,
            )
            .expect("wrapper analyze");
            assert_eq!(summary.windows, 2);
            assert_eq!(
                meter_calls.load(Ordering::SeqCst),
                1,
                "wrappers must not add a second meter"
            );

            // Second call via analyze_ecl must also meter exactly once more (fresh job).
            let csv2 = dir.path().join("full.csv");
            analyze_ecl(&ecl, &tiny_dynamic_fixture(), &csv2).expect("wrapper analyze");
            assert_eq!(
                meter_calls.load(Ordering::SeqCst),
                2,
                "each job meters once; wrappers must not double within a job"
            );
        });
    }

    // --- Batched inference loop, stage timings, observed entry (inference-acceleration 3.1) ---

    fn tiny_dynamic_fixture() -> PathBuf {
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests")
            .join("fixtures")
            .join("phase2_tiny_dynamic.onnx")
    }

    fn cpu_options(batch: usize) -> InferenceOptions {
        InferenceOptions {
            provider: ExecutionProviderKind::Cpu,
            batch_size: BatchSize::new(batch).expect("valid batch size"),
            cuda: CudaTuning::default(),
        }
    }

    fn load_tiny(batch: usize) -> Phase2Model {
        let model = Phase2Model::load_with_options(tiny_dynamic_fixture(), &cpu_options(batch))
            .expect("load tiny dynamic fixture on CPU");
        assert_eq!(model.batch_size(), batch);
        model
    }

    /// Spiky QRS-like pulses on a slow baseline so the tiny model yields beat peaks.
    fn synthetic_signal(windows: usize, abs_valid_start_500: i64) -> AiContinuousSignal {
        let len = WINDOW_SAMPLES + (windows - 1) * crate::preprocess::STEP_SAMPLES;
        let ecg_valid_500: Vec<f32> = (0..len)
            .map(|i| {
                let t = i as f32;
                let phase = (i % 397) as f32 - 198.0;
                let pulse = 6.0 * (-(phase * phase) / 18.0).exp();
                pulse + 0.3 * (t * 0.0021).sin() + 0.05 * (t * 0.37).cos()
            })
            .collect();
        let starts_abs_500 = (0..windows)
            .map(|k| abs_valid_start_500 + (k * crate::preprocess::STEP_SAMPLES) as i64)
            .collect();
        AiContinuousSignal {
            abs_valid_end_500: abs_valid_start_500 + len as i64,
            ecg_valid_500,
            starts_abs_500,
            abs_valid_start_500,
        }
    }

    type CandidateKey = (i64, usize, usize, u32, u32, u32, u32, u32);

    fn candidate_keys(candidates: &[crate::postprocess::BeatCandidate]) -> Vec<CandidateKey> {
        candidates
            .iter()
            .map(|c| {
                (
                    c.abs_pos,
                    c.local_pos,
                    c.window_index,
                    c.beat_score.to_bits(),
                    c.center_distance.to_bits(),
                    c.pac_score.to_bits(),
                    c.pvc_score.to_bits(),
                    c.n_score.to_bits(),
                )
            })
            .collect()
    }

    fn rhythm_keys(windows: &[RhythmWindow]) -> Vec<(i64, i64, u32)> {
        windows
            .iter()
            .map(|w| {
                (
                    w.start_sample_500,
                    w.end_sample_500,
                    w.rhythm_score.to_bits(),
                )
            })
            .collect()
    }

    #[test]
    fn infer_windows_batch_1_and_16_give_identical_candidates_and_rhythm_windows() {
        let total = 37;
        let signal = synthetic_signal(total, 1_234);
        let run = |batch: usize| {
            let mut model = load_tiny(batch);
            let mut seen = Vec::new();
            let result = infer_windows(
                &mut model,
                &signal,
                &signal.starts_abs_500,
                &mut |wi: usize, out: &WindowOutputs| {
                    assert_eq!(out.beat.len(), WINDOW_SAMPLES);
                    seen.push(wi);
                },
            )
            .expect("infer_windows");
            (result, seen)
        };

        let (one, seen_one) = run(1);
        let (sixteen, seen_sixteen) = run(16);

        let expected_order: Vec<usize> = (0..total).collect();
        assert_eq!(seen_one, expected_order, "batch 1 observer order");
        assert_eq!(seen_sixteen, expected_order, "batch 16 observer order");

        assert_eq!(one.rhythm_windows.len(), total);
        assert_eq!(sixteen.rhythm_windows.len(), total, "tail chunk kept");
        assert!(
            !one.candidates.is_empty(),
            "synthetic signal must produce beat candidates"
        );
        let windows_with_candidates: std::collections::BTreeSet<usize> =
            one.candidates.iter().map(|c| c.window_index).collect();
        assert!(
            windows_with_candidates.contains(&(total - 1)),
            "last (tail) window must contribute candidates"
        );

        assert_eq!(
            candidate_keys(&one.candidates),
            candidate_keys(&sixteen.candidates),
            "candidates must not depend on batch size"
        );
        assert_eq!(
            rhythm_keys(&one.rhythm_windows),
            rhythm_keys(&sixteen.rhythm_windows),
            "rhythm windows must not depend on batch size"
        );
        for (k, w) in sixteen.rhythm_windows.iter().enumerate() {
            assert_eq!(w.start_sample_500, signal.starts_abs_500[k]);
            assert_eq!(w.end_sample_500, w.start_sample_500 + WINDOW_SAMPLES as i64);
        }
    }

    #[test]
    fn infer_windows_matches_per_window_inference() {
        let total = 5;
        let signal = synthetic_signal(total, 0);
        let mut reference_model = load_tiny(1);
        let mut model = load_tiny(4);
        let mut observed = Vec::new();
        infer_windows(
            &mut model,
            &signal,
            &signal.starts_abs_500,
            &mut |wi: usize, out: &WindowOutputs| observed.push((wi, out.clone())),
        )
        .expect("infer_windows");
        assert_eq!(observed.len(), total);
        for (k, (wi, out)) in observed.iter().enumerate() {
            assert_eq!(*wi, k);
            let window = materialize_window(
                &signal.ecg_valid_500,
                signal.abs_valid_start_500,
                signal.starts_abs_500[k],
            );
            let want = reference_model.infer_window(&window).expect("infer_window");
            assert_eq!(out.beat, want.beat, "window {k}: beat");
            assert_eq!(out.event, want.event, "window {k}: event");
            assert_eq!(out.rhythm, want.rhythm, "window {k}: rhythm");
        }
    }

    /// 11-minute recording → 38 windows (chunks 16, 16, 6 at batch 16).
    const SYNTH_ECL_NAME: &str = "1234567890_20250101_0000_0011.ecl";
    const SYNTH_ECL_WINDOWS: usize = 38;

    /// 24 h ECL (minimum accepted size) with QRS-like pulses at 250 Hz.
    fn write_synthetic_ecl(dir: &Path) -> PathBuf {
        let n = crate::preprocess::EXPECTED_24H_SAMPLES_250;
        let mut bytes = Vec::with_capacity(n * 2);
        for i in 0..n {
            let phase = (i % 199) as f32 - 99.0;
            let value = 400.0 * (-(phase * phase) / 4.0).exp() + 20.0 * (i as f32 * 0.004).sin();
            let raw12 = (0x0800 + value.round() as i32).clamp(0, 0x0FFF) as u16;
            let word = ((raw12 & 0x0F00) << 4) | (raw12 & 0x00FF);
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        let path = dir.join(SYNTH_ECL_NAME);
        std::fs::write(&path, bytes).expect("write synthetic ECL");
        path
    }

    fn row_keys(rows: &[BeatResultRow]) -> Vec<String> {
        rows.iter()
            .map(|r| {
                format!(
                    "{}|{}|{}|{}|{}|{}|{}",
                    r.record_id,
                    r.beat_idx,
                    r.beat_time,
                    r.unknown,
                    r.beat_class,
                    r.rhythm_class,
                    r.short_run_flag
                )
            })
            .collect()
    }

    #[test]
    fn observed_entry_reports_all_windows_perf_and_same_rows_for_batch_1_and_16() {
        with_clean_global(|| {
            let meter_calls = Arc::new(AtomicUsize::new(0));
            LicenseGate::install(LicenseGate::new(CountingMeterClient::allow(Arc::clone(
                &meter_calls,
            ))))
            .expect("install counting gate");

            let dir = TempDir::new().expect("tempdir");
            let ecl = write_synthetic_ecl(dir.path());
            let run = |batch: usize| {
                let mut model = load_tiny(batch);
                let csv = dir.path().join(format!("out_{batch}.csv"));
                let mut seen = Vec::new();
                let (rows, summary) = analyze_ecl_with_model_observed(
                    &ecl,
                    &mut model,
                    &csv,
                    None,
                    &mut |wi: usize, _out: &WindowOutputs| seen.push(wi),
                )
                .expect("observed analyze");
                assert!(csv.exists());
                (rows, summary, seen, std::fs::read(&csv).expect("read csv"))
            };

            let (rows_one, summary_one, seen_one, csv_one) = run(1);
            let (rows_sixteen, summary_sixteen, seen_sixteen, csv_sixteen) = run(16);
            assert_eq!(meter_calls.load(Ordering::SeqCst), 2, "one meter per job");

            let expected_order: Vec<usize> = (0..SYNTH_ECL_WINDOWS).collect();
            assert_eq!(seen_one, expected_order);
            assert_eq!(seen_sixteen, expected_order);
            assert_eq!(summary_one.windows, SYNTH_ECL_WINDOWS);
            assert_eq!(summary_sixteen.windows, SYNTH_ECL_WINDOWS);
            assert!(!rows_one.is_empty(), "synthetic ECL must yield beats");
            assert_eq!(row_keys(&rows_one), row_keys(&rows_sixteen));
            assert_eq!(csv_one, csv_sixteen, "CSV bytes must not depend on batch");
            assert_eq!(summary_one.beats, summary_sixteen.beats);
            assert_eq!(summary_one.unknown_ones, summary_sixteen.unknown_ones);
            assert_eq!(summary_one.short_run_ones, summary_sixteen.short_run_ones);

            for (batch, summary) in [(1, &summary_one), (16, &summary_sixteen)] {
                let perf = &summary.perf;
                assert_eq!(perf.windows, SYNTH_ECL_WINDOWS, "batch {batch}");
                let eff = perf.effective.as_ref().expect("effective settings");
                assert_eq!(eff.batch_size, batch);
                assert_eq!(eff.provider, ExecutionProviderKind::Cpu);
                let t = &perf.timings;
                assert!(t.model_load.is_none(), "caller-loaded model: no model_load");
                assert!(t.preprocess > Duration::ZERO, "batch {batch}: {t:?}");
                assert!(t.inference > Duration::ZERO, "batch {batch}: {t:?}");
                assert!(t.output > Duration::ZERO, "batch {batch}: {t:?}");
                let stages = t.preprocess + t.inference + t.postprocess + t.output;
                assert!(t.total >= stages, "total covers all stages: {t:?}");
            }
        });
    }

    #[test]
    fn canonical_entry_records_model_load_and_effective_batch() {
        with_clean_global(|| {
            install_allow_gate();
            let dir = TempDir::new().expect("tempdir");
            let ecl = write_synthetic_ecl(dir.path());
            let csv = dir.path().join("out.csv");
            let (rows, summary) = analyze_ecl_with_source(
                &ecl,
                &ModelSource::Path(tiny_dynamic_fixture()),
                &csv,
                Some(20),
                &cpu_options(16),
            )
            .expect("canonical analyze with tiny fixture");
            assert_eq!(summary.windows, 20);
            assert_eq!(rows.len(), summary.beats);
            let perf = &summary.perf;
            assert_eq!(perf.windows, 20);
            assert_eq!(perf.effective.as_ref().map(|e| e.batch_size), Some(16));
            let t = &perf.timings;
            let model_load = t.model_load.expect("canonical entry loads the model");
            let stages = model_load + t.preprocess + t.inference + t.postprocess + t.output;
            assert!(t.total >= stages, "total covers model load too: {t:?}");
        });
    }

    #[test]
    fn observed_entry_uninstalled_gate_fail_closed_without_windows_or_output() {
        with_clean_global(|| {
            let dir = TempDir::new().expect("tempdir");
            let ecl = write_synthetic_ecl(dir.path());
            let csv = dir.path().join("out.csv");
            let mut model = load_tiny(4);
            let mut calls = 0usize;
            let err = analyze_ecl_with_model_observed(
                &ecl,
                &mut model,
                &csv,
                Some(3),
                &mut |_wi: usize, _out: &WindowOutputs| calls += 1,
            )
            .expect_err("uninstalled gate must fail-closed");
            assert!(
                matches!(
                    &err,
                    AnalyzeError::License(LicenseError::InferenceDenied(f))
                        if f.reason == LicenseFailureReason::GateNotInstalled
                ),
                "expected License(InferenceDenied(gate_not_installed)), got {err:?}"
            );
            assert_eq!(calls, 0, "no window may be observed without license");
            assert!(!csv.exists());
        });
    }

    // --- Metering after preprocess, before the first window inference (task 10.1) ---

    #[test]
    fn invalid_ecl_filename_is_not_metered() {
        with_clean_global(|| {
            let meter_calls = install_counting_gate();
            let dir = TempDir::new().expect("tempdir");
            let ecl = write_synthetic_ecl(dir.path());
            let renamed = dir.path().join("not_an_ecl_name.ecl");
            std::fs::rename(&ecl, &renamed).expect("rename synthetic ECL");
            let csv = dir.path().join("out.csv");

            let err = analyze_ecl_with_source(
                &renamed,
                &ModelSource::Path(tiny_dynamic_fixture()),
                &csv,
                Some(2),
                &cpu_options(4),
            )
            .expect_err("invalid filename must fail");
            assert!(matches!(err, AnalyzeError::Preprocess(_)), "got {err:?}");

            let mut model = load_tiny(4);
            let err = analyze_ecl_with_model(&renamed, &mut model, &csv, Some(2))
                .expect_err("invalid filename must fail with resident model");
            assert!(matches!(err, AnalyzeError::Preprocess(_)), "got {err:?}");

            assert_eq!(meter_calls.load(Ordering::SeqCst), 0);
            assert!(!csv.exists());
        });
    }

    #[test]
    fn unreadable_ecl_is_not_metered() {
        with_clean_global(|| {
            let meter_calls = install_counting_gate();
            let dir = TempDir::new().expect("tempdir");
            let missing_ecl = dir.path().join(SYNTH_ECL_NAME);
            let short_ecl = dir.path().join("1234567890_20250102_0000_0011.ecl");
            std::fs::write(&short_ecl, b"placeholder").expect("write short ECL");
            let csv = dir.path().join("out.csv");

            for ecl in [&missing_ecl, &short_ecl] {
                let err = analyze_ecl_with_source(
                    ecl,
                    &ModelSource::Path(tiny_dynamic_fixture()),
                    &csv,
                    Some(2),
                    &cpu_options(4),
                )
                .expect_err("unreadable ECL must fail");
                assert!(matches!(err, AnalyzeError::Preprocess(_)), "got {err:?}");
            }

            assert_eq!(meter_calls.load(Ordering::SeqCst), 0);
            assert!(!csv.exists());
        });
    }

    #[test]
    fn missing_model_is_not_metered() {
        with_clean_global(|| {
            let meter_calls = install_counting_gate();
            let dir = TempDir::new().expect("tempdir");
            let ecl = write_synthetic_ecl(dir.path());
            let csv = dir.path().join("out.csv");

            let err = analyze_ecl_with_source(
                &ecl,
                &ModelSource::Path(dir.path().join("missing.onnx")),
                &csv,
                Some(2),
                &cpu_options(4),
            )
            .expect_err("missing model must fail");
            assert!(
                matches!(err, AnalyzeError::Infer(InferError::ModelNotFound(_))),
                "got {err:?}"
            );

            assert_eq!(meter_calls.load(Ordering::SeqCst), 0);
            assert!(!csv.exists());
        });
    }

    #[test]
    fn analyze_ecl_with_model_meters_once_per_job() {
        with_clean_global(|| {
            let meter_calls = Arc::new(AtomicUsize::new(0));
            LicenseGate::install(LicenseGate::new(CountingMeterClient::allow(Arc::clone(
                &meter_calls,
            ))))
            .expect("install counting gate");
            let dir = TempDir::new().expect("tempdir");
            let ecl = write_synthetic_ecl(dir.path());
            let csv = dir.path().join("out.csv");
            let mut model = load_tiny(16);
            let (_rows, summary) =
                analyze_ecl_with_model(&ecl, &mut model, &csv, Some(17)).expect("analyze");
            assert_eq!(summary.windows, 17);
            assert!(summary.perf.timings.model_load.is_none());
            assert_eq!(meter_calls.load(Ordering::SeqCst), 1);
        });
    }
}
