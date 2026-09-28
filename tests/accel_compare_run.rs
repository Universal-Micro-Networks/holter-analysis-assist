//! Baseline vs candidate comparison run (inference-acceleration task 3.6,
//! requirements 2.5, 5.1, 5.3, 5.7).
//!
//! The license gate is process-wide, so tests in this binary share one
//! counting allow gate and run one at a time.

use holter_analysis_assist::accel_compare::report::{
    Thresholds, EXIT_SUCCESS, EXIT_THRESHOLD_FAILED, REPORT_JSON, REPORT_MARKDOWN,
};
use holter_analysis_assist::accel_compare::{
    run_compare, CompareConfig, CompareError, BASELINE_CSV, CANDIDATE_CSV, DEFAULT_PROB_STRIDE,
    DEFAULT_TOLERANCE_SAMPLES,
};
use holter_analysis_assist::inference_options::{BatchSize, CudaTuning};
use holter_analysis_assist::license::{
    LicenseCheckResult, LicenseClient, LicenseError, LicenseGate, LicenseMeterResult,
};
use holter_analysis_assist::phase2::{
    ExecutionProviderKind, InferError, InferenceOptions, Phase2Model,
};
use holter_analysis_assist::preprocess::EXPECTED_24H_SAMPLES_250;
use holter_analysis_assist::ModelSource;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Mutex, MutexGuard, Once};
use tempfile::TempDir;

static METER_CALLS: AtomicUsize = AtomicUsize::new(0);
static SERIAL: Mutex<()> = Mutex::new(());

struct CountingAllowClient;

impl LicenseClient for CountingAllowClient {
    fn check_validity(&self) -> Result<LicenseCheckResult, LicenseError> {
        Ok(LicenseCheckResult {
            allowed: true,
            ..Default::default()
        })
    }

    fn authorize_and_meter(&self) -> Result<LicenseMeterResult, LicenseError> {
        METER_CALLS.fetch_add(1, Ordering::SeqCst);
        Ok(LicenseMeterResult {
            allowed: true,
            ..Default::default()
        })
    }
}

/// Serializes the test and resets the meter count.
fn serial_with_gate() -> MutexGuard<'static, ()> {
    static INSTALL: Once = Once::new();
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    INSTALL.call_once(|| {
        LicenseGate::install(LicenseGate::new(CountingAllowClient)).expect("install gate");
    });
    METER_CALLS.store(0, Ordering::SeqCst);
    guard
}

fn meter_calls() -> usize {
    METER_CALLS.load(Ordering::SeqCst)
}

fn tiny_dynamic_fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("phase2_tiny_dynamic.onnx")
}

fn options(provider: ExecutionProviderKind, batch: usize) -> InferenceOptions {
    InferenceOptions {
        provider,
        batch_size: BatchSize::new(batch).expect("valid batch size"),
        cuda: CudaTuning::default(),
    }
}

fn config(
    ecls: Vec<PathBuf>,
    model: PathBuf,
    candidate: InferenceOptions,
    dir: &Path,
) -> CompareConfig {
    CompareConfig {
        ecl_paths: ecls,
        model: ModelSource::Path(model),
        candidate,
        tolerance_samples: DEFAULT_TOLERANCE_SAMPLES,
        prob_stride: DEFAULT_PROB_STRIDE,
        max_windows: None,
        thresholds: Thresholds::default(),
        report_dir: dir.join("report"),
    }
}

/// Valid ECL filename; the file is never read when the run aborts first.
fn stub_ecl() -> PathBuf {
    PathBuf::from("1234567890_20250101_0000_2359.ecl")
}

/// 11-minute recording → 38 windows.
const SYNTH_ECL_NAME: &str = "1234567890_20250101_0000_0011.ecl";
const SYNTH_ECL_STEM: &str = "1234567890_20250101_0000_0011";
const SYNTH_ECL_WINDOWS: usize = 38;

/// 24 h ECL (minimum accepted size) with QRS-like pulses at 250 Hz.
fn write_synthetic_ecl(dir: &Path) -> PathBuf {
    let n = EXPECTED_24H_SAMPLES_250;
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

fn assert_candidate_unavailable_before_any_work(
    result: Result<impl std::fmt::Debug, CompareError>,
    report_dir: &Path,
) -> InferError {
    let err = result.expect_err("candidate load failure must abort the comparison");
    let msg = err.to_string();
    let CompareError::CandidateUnavailable(inner) = err else {
        panic!("expected CandidateUnavailable, got {err:?}");
    };
    assert!(msg.contains("candidate provider unavailable"), "{msg}");
    assert!(
        msg.contains(&inner.to_string()),
        "reason must be shown: {msg}"
    );
    assert_eq!(meter_calls(), 0, "no analysis may run (or be metered)");
    assert!(!report_dir.exists(), "nothing may be written");
    inner
}

/// Requirement 5.7: on a machine without a usable CUDA EP (or a build
/// without the `cuda` feature) a CUDA candidate aborts the comparison.
#[test]
fn cuda_candidate_aborts_when_cuda_is_unavailable() {
    let _guard = serial_with_gate();
    let cuda = options(ExecutionProviderKind::Cuda, 16);
    if Phase2Model::load_with_options(tiny_dynamic_fixture(), &cuda).is_ok() {
        eprintln!("skip: CUDA provider is available on this machine");
        return;
    }
    let dir = TempDir::new().expect("tempdir");
    let cfg = config(vec![stub_ecl()], tiny_dynamic_fixture(), cuda, dir.path());
    let inner = assert_candidate_unavailable_before_any_work(run_compare(&cfg), &cfg.report_dir);
    assert!(
        !matches!(inner, InferError::ModelNotFound(_)),
        "the fixture exists; the provider must be the reason: {inner}"
    );
}

/// Counterpart of the test above on machines with a usable CUDA EP.
#[test]
fn cuda_candidate_runs_when_cuda_is_available() {
    let _guard = serial_with_gate();
    let cuda = options(ExecutionProviderKind::Cuda, 16);
    if let Err(e) = Phase2Model::load_with_options(tiny_dynamic_fixture(), &cuda) {
        eprintln!("skip: CUDA provider unavailable ({e})");
        return;
    }
    let dir = TempDir::new().expect("tempdir");
    let ecl = write_synthetic_ecl(dir.path());
    let cfg = config(vec![ecl], tiny_dynamic_fixture(), cuda, dir.path());
    let report = run_compare(&cfg).expect("compare run with CUDA candidate");
    assert_eq!(meter_calls(), 2);
    assert_eq!(report.baseline.provider, "cpu");
    assert_eq!(report.candidate.provider, "cuda");
    assert_eq!(report.candidate.batch_size, 16);
    assert_eq!(report.candidate.cuda_applied, Some(true));
    let a = &report.files[0].accuracy;
    assert_eq!(a.windows, SYNTH_ECL_WINDOWS);
    assert_eq!(a.prob_max_abs_diff.sampled_windows, SYNTH_ECL_WINDOWS);
    let p = &a.prob_max_abs_diff;
    for diff in [p.beat, p.event_pac, p.event_pvc, p.event_n, p.rhythm] {
        assert!(diff < 1e-4, "{p:?}");
    }
}

#[test]
fn candidate_load_failure_aborts_before_baseline_and_metering() {
    let _guard = serial_with_gate();
    let dir = TempDir::new().expect("tempdir");
    let missing = dir.path().join("missing-model.onnx");
    let cfg = config(
        vec![stub_ecl()],
        missing,
        options(ExecutionProviderKind::Cpu, 16),
        dir.path(),
    );
    let inner = assert_candidate_unavailable_before_any_work(run_compare(&cfg), &cfg.report_dir);
    assert!(matches!(inner, InferError::ModelNotFound(_)), "{inner}");
}

#[test]
fn invalid_config_is_rejected_before_any_work() {
    let _guard = serial_with_gate();
    let dir = TempDir::new().expect("tempdir");
    let cpu16 = options(ExecutionProviderKind::Cpu, 16);

    let no_ecl = config(Vec::new(), tiny_dynamic_fixture(), cpu16, dir.path());
    let err = run_compare(&no_ecl).expect_err("no ECL");
    assert!(matches!(err, CompareError::Config(_)), "{err:?}");

    let mut zero_stride = config(vec![stub_ecl()], tiny_dynamic_fixture(), cpu16, dir.path());
    zero_stride.prob_stride = 0;
    let err = run_compare(&zero_stride).expect_err("prob_stride 0");
    assert!(
        matches!(&err, CompareError::Config(m) if m.contains("prob_stride")),
        "{err:?}"
    );

    assert_eq!(meter_calls(), 0);
    assert!(!dir.path().join("report").exists());
}

/// Requirements 2.5 / 5.1 / 5.3: CPU batch 16 vs the CPU batch 1 baseline
/// agrees completely; per-ECL CSVs and both report files are written.
#[test]
fn cpu_batch16_candidate_matches_baseline_and_writes_all_outputs() {
    let _guard = serial_with_gate();
    let dir = TempDir::new().expect("tempdir");
    let ecl = write_synthetic_ecl(dir.path());
    // The same ECL twice: two files in the report, CSVs must not collide.
    let cfg = config(
        vec![ecl.clone(), ecl],
        tiny_dynamic_fixture(),
        options(ExecutionProviderKind::Cpu, 16),
        dir.path(),
    );
    let report = run_compare(&cfg).expect("compare run");

    assert_eq!(meter_calls(), 4, "baseline + candidate per ECL");

    assert_eq!(report.baseline.provider, "cpu");
    assert_eq!(report.baseline.batch_size, 1);
    assert_eq!(
        report.baseline.cuda_tuning,
        "tf32=default,conv1d_pad_to_nc1d=default,cuda_graph=default"
    );
    assert_eq!(report.candidate.provider, "cpu");
    assert_eq!(report.candidate.batch_size, 16);
    assert_eq!(report.candidate.model_batch.as_deref(), Some("dynamic"));
    assert_eq!(report.tolerance_samples, DEFAULT_TOLERANCE_SAMPLES);
    assert_eq!(report.verdict, None);
    assert_eq!(report.exit_code(), EXIT_SUCCESS);

    assert_eq!(report.files.len(), 2);
    for file in &report.files {
        let a = &file.accuracy;
        assert_eq!(a.windows, SYNTH_ECL_WINDOWS);
        assert!(a.baseline_beats > 0, "synthetic ECL must yield beats");
        assert_eq!(a.candidate_beats, a.baseline_beats);
        assert_eq!(a.matched_beats, a.baseline_beats);
        assert_eq!(a.beat_count_diff, 0);
        assert_eq!(a.offset_max_samples, 0.0);
        for rate in [
            a.rhythm_window_agreement,
            a.match_rate_vs_baseline,
            a.match_rate_vs_candidate,
            a.beat_class_agreement,
            a.rhythm_class_agreement,
            a.unknown_agreement,
            a.short_run_agreement,
        ] {
            assert_eq!(rate, 1.0);
        }
        let p = &a.prob_max_abs_diff;
        assert_eq!(p.sampled_windows, SYNTH_ECL_WINDOWS, "prob_stride 1");
        // x86 ONNX Runtime kernels vary by batch size; only last-bit noise is allowed.
        for diff in [p.beat, p.event_pac, p.event_pvc, p.event_n, p.rhythm] {
            assert!(diff <= 1e-5, "{p:?}");
        }

        for perf in [&file.baseline_perf, &file.candidate_perf] {
            assert_eq!(perf.windows, SYNTH_ECL_WINDOWS);
            assert!(perf.inference_ms > 0.0, "{perf:?}");
            assert!(perf.total_ms >= perf.inference_ms, "{perf:?}");
            assert!(perf.windows_per_s_total > 0.0, "{perf:?}");
            assert!(perf.windows_per_s_inference > 0.0, "{perf:?}");
        }
    }
    let agg = &report.aggregate;
    assert_eq!(agg.files, 2);
    assert_eq!(agg.accuracy.windows, 2 * SYNTH_ECL_WINDOWS);
    assert_eq!(agg.baseline_perf.windows, 2 * SYNTH_ECL_WINDOWS);
    assert!(agg.speedup_total.is_some());

    let report_dir = &cfg.report_dir;
    let json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(report_dir.join(REPORT_JSON)).expect("report.json"),
    )
    .expect("valid JSON");
    assert_eq!(json["candidate"]["batch_size"], 16);
    assert_eq!(json["baseline"]["batch_size"], 1);
    assert_eq!(json["files"].as_array().map(Vec::len), Some(2));
    assert!(json["verdict"].is_null());
    let md = std::fs::read_to_string(report_dir.join(REPORT_MARKDOWN)).expect("report.md");
    assert!(md.contains("# 推論高速化 比較レポート"), "{md}");

    let mut csv_dirs: Vec<PathBuf> = std::fs::read_dir(report_dir)
        .expect("read report dir")
        .map(|e| e.expect("dir entry").path())
        .filter(|p| p.is_dir())
        .collect();
    csv_dirs.sort();
    assert_eq!(
        csv_dirs,
        vec![
            report_dir.join(SYNTH_ECL_STEM),
            report_dir.join(format!("{SYNTH_ECL_STEM}_2")),
        ]
    );
    for d in &csv_dirs {
        let baseline = std::fs::read(d.join(BASELINE_CSV)).expect("baseline.csv");
        let candidate = std::fs::read(d.join(CANDIDATE_CSV)).expect("candidate.csv");
        assert!(!baseline.is_empty());
        assert_eq!(
            baseline,
            candidate,
            "{}: CSV must not depend on batch size",
            d.display()
        );
    }
}

#[test]
fn prob_stride_and_max_windows_limit_sampling_and_thresholds_are_judged() {
    let _guard = serial_with_gate();
    let dir = TempDir::new().expect("tempdir");
    let ecl = write_synthetic_ecl(dir.path());
    let mut cfg = config(
        vec![ecl],
        tiny_dynamic_fixture(),
        options(ExecutionProviderKind::Cpu, 4),
        dir.path(),
    );
    cfg.prob_stride = 5;
    cfg.max_windows = Some(12);
    cfg.thresholds = Thresholds {
        max_prob_abs_diff: Some(0.0),
        min_beat_match_rate: Some(1.5),
        ..Thresholds::default()
    };
    let report = run_compare(&cfg).expect("compare run");
    assert_eq!(meter_calls(), 2);

    let a = &report.files[0].accuracy;
    assert_eq!(a.windows, 12, "rhythm is compared on every window");
    assert_eq!(a.prob_max_abs_diff.sampled_windows, 3, "windows 0, 5, 10");
    assert_eq!(report.files[0].candidate_perf.windows, 12);

    let verdict = report.verdict.as_ref().expect("thresholds given");
    let passed: Vec<(&str, bool)> = verdict
        .checks
        .iter()
        .map(|c| (c.metric.as_str(), c.passed))
        .collect();
    assert_eq!(
        passed,
        [("min_beat_match_rate", false), ("max_prob_abs_diff", true)]
    );
    assert_eq!(report.exit_code(), EXIT_THRESHOLD_FAILED);
    let json: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(cfg.report_dir.join(REPORT_JSON)).expect("report.json"),
    )
    .expect("valid JSON");
    assert_eq!(json["verdict"]["passed"], false);
}

/// `resources/samples/sample.ecl`, else the first `*.ecl` there.
fn real_sample_ecl() -> Option<PathBuf> {
    let samples = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("samples");
    let link = samples.join("sample.ecl");
    if link.exists() {
        // Symlink basename is `sample.ecl`; resolve to the real file whose name matches ECL rules.
        return link.canonicalize().ok();
    }
    let mut ecls: Vec<PathBuf> = std::fs::read_dir(&samples)
        .ok()?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("ecl")))
        .collect();
    ecls.sort();
    ecls.into_iter().next()
}

/// Real ECL + real model: CPU batch 16 agrees with the CPU batch 1 baseline.
#[test]
fn real_ecl_cpu_batch16_matches_baseline() {
    let _guard = serial_with_gate();
    let onnx = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("resources")
        .join("models")
        .join("phase2_rev1.onnx");
    let Some(ecl) = real_sample_ecl().filter(|_| onnx.exists()) else {
        eprintln!("skip: sample ECL or ONNX not present");
        return;
    };
    let dir = TempDir::new().expect("tempdir");
    let mut cfg = config(
        vec![ecl],
        onnx,
        options(ExecutionProviderKind::Cpu, 16),
        dir.path(),
    );
    cfg.max_windows = Some(6);
    let report = run_compare(&cfg).expect("compare run on real ECL");
    assert_eq!(meter_calls(), 2);

    let a = &report.files[0].accuracy;
    assert_eq!(a.windows, 6);
    assert_eq!(a.prob_max_abs_diff.sampled_windows, 6);
    assert_eq!(a.candidate_beats, a.baseline_beats);
    assert_eq!(a.matched_beats, a.baseline_beats);
    for rate in [
        a.rhythm_window_agreement,
        a.beat_class_agreement,
        a.rhythm_class_agreement,
        a.unknown_agreement,
        a.short_run_agreement,
    ] {
        assert_eq!(rate, 1.0, "{a:?}");
    }
    let p = &a.prob_max_abs_diff;
    for diff in [p.beat, p.event_pac, p.event_pvc, p.event_n, p.rhythm] {
        assert!(diff < 1e-4, "{p:?}");
    }
    assert!(cfg.report_dir.join(REPORT_JSON).exists());
    assert!(cfg.report_dir.join(REPORT_MARKDOWN).exists());
}
