//! End-to-end ECL analysis pipeline (BeatSense `analyzer.py` Rust port).

use crate::model_source::ModelSource;
use crate::phase2::{ExecutionProviderKind, Phase2Model, WINDOW_SAMPLES};
use crate::postprocess::{
    build_rhythm_intervals, build_unknown_intervals, center_best_beats, classify_event_sigmoid,
    cluster_candidates, complement_intervals, detect_run_candidates, extract_window_candidates,
    finalize_runs_after_unknown, label_points_by_rhythm, point_in_intervals, RhythmWindow,
};
use crate::preprocess::{
    build_ai_continuous_signal, materialize_window, parse_ecl_filename, read_ecl_adc_counts, FS,
};
use chrono::{Duration, NaiveDateTime};
use csv::WriterBuilder;
use serde::Serialize;
use std::path::Path;
use thiserror::Error;

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
        ExecutionProviderKind::Auto,
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
        provider,
    )
}

/// Canonical ECL analysis entry: load the model via [`ModelSource`], then run the pipeline.
///
/// License authorize+meter runs once at the start via the process-wide [`crate::license::LicenseGate`].
pub fn analyze_ecl_with_source(
    ecl_path: &Path,
    model: &ModelSource,
    output_csv: &Path,
    max_windows: Option<usize>,
    provider: ExecutionProviderKind,
) -> Result<(Vec<BeatResultRow>, AnalyzeSummary), AnalyzeError> {
    // Fail-closed: uninstalled gate or meter deny rejects before any analyze output.
    match crate::license::LicenseGate::try_global() {
        None => {
            return Err(crate::license::LicenseError::InferenceDenied(
                "license gate not installed".into(),
            )
            .into());
        }
        Some(gate) => {
            gate.ensure_inference_allowed()?;
        }
    }

    let source_info = parse_ecl_filename(ecl_path)?;
    // Fail-fast on model source before reading the full ECL (Path missing / Embedded unavailable).
    let mut model = Phase2Model::load_from_source(model, provider)?;

    let ecg_all_250 = read_ecl_adc_counts(ecl_path)?;
    eprintln!("[1/6] AI preprocessing ...");
    let signal = build_ai_continuous_signal(&ecg_all_250, &source_info)?;

    let mut starts = signal.starts_abs_500.clone();
    if let Some(limit) = max_windows {
        starts.truncate(limit);
    }

    eprintln!(
        "[2/6] ONNX inference: windows={} requested_provider={} using_provider={}",
        starts.len(),
        provider,
        model.provider()
    );
    let mut all_candidates = Vec::new();
    let mut rhythm_windows = Vec::new();

    for (wi, &start_abs) in starts.iter().enumerate() {
        let window =
            materialize_window(&signal.ecg_valid_500, signal.abs_valid_start_500, start_abs);
        let out = model.infer_window(&window)?;
        all_candidates.extend(extract_window_candidates(
            &out.beat, &out.event, start_abs, wi,
        ));
        rhythm_windows.push(RhythmWindow {
            start_sample_500: start_abs,
            end_sample_500: start_abs + WINDOW_SAMPLES as i64,
            rhythm_score: out.rhythm,
        });
        if wi > 0 && wi % 200 == 0 {
            eprintln!("      ... window {wi}/{}", starts.len());
        }
    }

    eprintln!("[3/6] Beat / event / rhythm post-processing ...");
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

    eprintln!("[6/6] Final CSV ...");
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

    let summary = AnalyzeSummary {
        beats: rows.len(),
        unknown_ones: rows.iter().filter(|r| r.unknown == 1).count(),
        short_run_ones: rows.iter().filter(|r| r.short_run_flag == 1).count(),
        windows: starts.len(),
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
    use crate::license::{
        LicenseCheckResult, LicenseClient, LicenseError, LicenseGate, LicenseMeterResult,
        MockLicenseClient, MockOutcome, GLOBAL_TEST_LOCK,
    };
    use crate::model_source::ModelSource;
    use crate::phase2::InferError;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
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

    #[test]
    fn analyze_ecl_with_source_missing_path_errors_before_ecl_read() {
        with_clean_global(|| {
            install_allow_gate();
            let (_dir, csv) = temp_csv();
            let missing = PathBuf::from("/tmp/holter-assist-missing-model-3-1.onnx");
            let source = ModelSource::Path(missing.clone());
            let err = analyze_ecl_with_source(
                &stub_ecl_path(),
                &source,
                &csv,
                Some(0),
                ExecutionProviderKind::Cpu,
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
        });
    }

    #[test]
    fn analyze_ecl_with_limit_wrapper_delegates_path_source() {
        with_clean_global(|| {
            install_allow_gate();
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
                ExecutionProviderKind::Cpu,
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
                ExecutionProviderKind::Cpu,
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
                ExecutionProviderKind::Cpu,
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
            let (_dir, csv) = temp_csv();
            let missing = PathBuf::from("/tmp/holter-assist-missing-license-uninstalled.onnx");
            let err = analyze_ecl_with_source(
                &stub_ecl_path(),
                &ModelSource::Path(missing),
                &csv,
                Some(0),
                ExecutionProviderKind::Cpu,
            )
            .expect_err("uninstalled gate must fail-closed");
            let msg = err.to_string();
            assert!(
                msg.contains("inference denied"),
                "must surface inference denial: {msg}"
            );
            assert!(
                matches!(err, AnalyzeError::License(LicenseError::InferenceDenied(_))),
                "expected License(InferenceDenied), got {err:?}"
            );
            assert!(!csv.exists(), "must not write CSV when gate missing");
        });
    }

    #[test]
    fn analyze_ecl_with_source_meter_deny_no_output_or_rows() {
        with_clean_global(|| {
            LicenseGate::install(LicenseGate::new(MockLicenseClient::new(
                MockOutcome::Success { message: None },
                MockOutcome::Deny {
                    message: Some("quota exceeded".into()),
                },
            )))
            .expect("install deny gate");

            let (_dir, csv) = temp_csv();
            let missing = PathBuf::from("/tmp/holter-assist-missing-license-deny.onnx");
            let err = analyze_ecl_with_source(
                &stub_ecl_path(),
                &ModelSource::Path(missing),
                &csv,
                Some(2),
                ExecutionProviderKind::Cpu,
            )
            .expect_err("meter deny must reject inference");
            let msg = err.to_string();
            assert!(
                msg.contains("inference denied") && msg.contains("quota exceeded"),
                "must identify inference denial: {msg}"
            );
            assert!(
                matches!(err, AnalyzeError::License(LicenseError::InferenceDenied(_))),
                "expected License(InferenceDenied), got {err:?}"
            );
            assert!(!csv.exists(), "must not write CSV on meter deny");
        });
    }

    #[test]
    fn analyze_ecl_with_source_meters_once_even_with_multiple_windows() {
        with_clean_global(|| {
            let meter_calls = Arc::new(AtomicUsize::new(0));
            LicenseGate::install(LicenseGate::new(CountingMeterClient::allow(Arc::clone(
                &meter_calls,
            ))))
            .expect("install counting gate");

            let sample_link = Path::new("resources/samples/sample.ecl");
            let onnx = Path::new("resources/models/phase2_rev1.onnx");
            if !sample_link.exists() || !onnx.exists() {
                // Still prove the entry meters once before model load (no window loop).
                let (_dir, csv) = temp_csv();
                let missing = PathBuf::from("/tmp/holter-assist-missing-license-multi-win.onnx");
                let _ = analyze_ecl_with_source(
                    &stub_ecl_path(),
                    &ModelSource::Path(missing),
                    &csv,
                    Some(3),
                    ExecutionProviderKind::Cpu,
                );
                assert_eq!(
                    meter_calls.load(Ordering::SeqCst),
                    1,
                    "canonical entry must meter once before model/window work"
                );
                return;
            }
            let sample = sample_link
                .canonicalize()
                .expect("canonicalize sample.ecl");
            let (_dir, csv) = temp_csv();
            let (rows, summary) = analyze_ecl_with_source(
                &sample,
                &ModelSource::Path(onnx.to_path_buf()),
                &csv,
                Some(2),
                ExecutionProviderKind::Cpu,
            )
            .expect("meter success should allow analyze");
            assert_eq!(summary.windows, 2, "exercise multiple windows");
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
            let meter_calls = Arc::new(AtomicUsize::new(0));
            LicenseGate::install(LicenseGate::new(CountingMeterClient::allow(Arc::clone(
                &meter_calls,
            ))))
            .expect("install counting gate");

            let (_dir, csv) = temp_csv();
            let missing = PathBuf::from("/tmp/holter-assist-missing-license-wrapper.onnx");
            let err = analyze_ecl_with_limit(
                &stub_ecl_path(),
                &missing,
                &csv,
                Some(2),
                ExecutionProviderKind::Cpu,
            )
            .expect_err("missing model after meter");
            assert!(
                matches!(err, AnalyzeError::Infer(InferError::ModelNotFound(_))),
                "wrapper should reach model load after single meter: {err}"
            );
            assert_eq!(
                meter_calls.load(Ordering::SeqCst),
                1,
                "wrappers must not add a second meter"
            );

            // Second call via analyze_ecl must also meter exactly once more (fresh job).
            let (_dir2, csv2) = temp_csv();
            let _ = analyze_ecl(
                &stub_ecl_path(),
                &PathBuf::from("/tmp/holter-assist-missing-license-wrapper2.onnx"),
                &csv2,
            );
            assert_eq!(
                meter_calls.load(Ordering::SeqCst),
                2,
                "each job meters once; wrappers must not double within a job"
            );
        });
    }
}
