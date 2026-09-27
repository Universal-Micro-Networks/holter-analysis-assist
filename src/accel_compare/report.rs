//! Comparison report output: machine-readable JSON and a human-readable
//! Markdown summary, plus pass/fail verdicts evaluated only when thresholds
//! are explicitly given (no thresholds are fixed here).
//!
//! Report assembly from observed data (beat rows, window accumulator,
//! perf measurements) is pure; only [`write_report`] touches the file
//! system.

use super::metrics::{
    aggregate, compute_accuracy, AccuracyMetrics, MetricsError, ProbDiff, WindowAccumulator,
};
use crate::analyze::BeatResultRow;
use crate::perf::AnalyzePerf;
use crate::phase2::{EffectiveInference, InferenceOptions, ModelBatchShape};
use crate::preprocess::FS;
use serde::Serialize;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

pub const EXIT_SUCCESS: i32 = 0;
/// At least one given threshold failed.
pub const EXIT_THRESHOLD_FAILED: i32 = 2;
pub const REPORT_JSON: &str = "report.json";
pub const REPORT_MARKDOWN: &str = "report.md";

/// Shown in the Markdown summary for rates / values whose denominator is
/// zero (reported as 1.0 or 0.0 in [`AccuracyMetrics`]).
const NO_COMPARISON: &str = "比較対象なし";
const ABSENT: &str = "-";

/// Optional acceptance thresholds, judged against the aggregate metrics.
/// There are no default values: an unset threshold is not checked.
///
/// Rates with nothing to compare are 1.0 in [`AccuracyMetrics`] and so pass
/// minimum thresholds.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize)]
pub struct Thresholds {
    pub min_rhythm_window_agreement: Option<f64>,
    /// Judged on the lower of `match_rate_vs_baseline` and
    /// `match_rate_vs_candidate`, so both missed and extra beats count.
    pub min_beat_match_rate: Option<f64>,
    pub min_beat_class_agreement: Option<f64>,
    /// Judged on the largest of the beat / event / rhythm differences.
    pub max_prob_abs_diff: Option<f64>,
    /// Judged on `offset_max_samples`.
    pub max_offset_samples: Option<f64>,
}

#[derive(Clone, Copy)]
enum Bound {
    Min,
    Max,
}

impl Thresholds {
    pub fn is_empty(&self) -> bool {
        self.min_rhythm_window_agreement.is_none()
            && self.min_beat_match_rate.is_none()
            && self.min_beat_class_agreement.is_none()
            && self.max_prob_abs_diff.is_none()
            && self.max_offset_samples.is_none()
    }

    /// Checks only the given thresholds; `None` when none is given.
    pub fn evaluate(&self, aggregate: &AccuracyMetrics) -> Option<Verdict> {
        let m = aggregate;
        let candidates = [
            (
                "min_rhythm_window_agreement",
                Bound::Min,
                self.min_rhythm_window_agreement,
                m.rhythm_window_agreement,
            ),
            (
                "min_beat_match_rate",
                Bound::Min,
                self.min_beat_match_rate,
                nan_min(m.match_rate_vs_baseline, m.match_rate_vs_candidate),
            ),
            (
                "min_beat_class_agreement",
                Bound::Min,
                self.min_beat_class_agreement,
                m.beat_class_agreement,
            ),
            (
                "max_prob_abs_diff",
                Bound::Max,
                self.max_prob_abs_diff,
                largest_prob_diff(&m.prob_max_abs_diff),
            ),
            (
                "max_offset_samples",
                Bound::Max,
                self.max_offset_samples,
                m.offset_max_samples,
            ),
        ];
        let checks: Vec<ThresholdCheck> = candidates
            .into_iter()
            .filter_map(|(metric, bound, threshold, actual)| {
                let threshold = threshold?;
                // Comparisons are false for NaN, so a NaN metric fails.
                let passed = match bound {
                    Bound::Min => actual >= threshold,
                    Bound::Max => actual <= threshold,
                };
                Some(ThresholdCheck {
                    metric: metric.to_string(),
                    actual,
                    threshold,
                    passed,
                })
            })
            .collect();
        if checks.is_empty() {
            return None;
        }
        let passed = checks.iter().all(|c| c.passed);
        Some(Verdict { checks, passed })
    }
}

/// One threshold check: `metric` is the threshold name (e.g.
/// `min_beat_match_rate`); `min_*` pass when `actual >= threshold`, `max_*`
/// when `actual <= threshold`.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ThresholdCheck {
    pub metric: String,
    pub actual: f64,
    pub threshold: f64,
    pub passed: bool,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Verdict {
    pub checks: Vec<ThresholdCheck>,
    pub passed: bool,
}

/// Inference settings of one side of the comparison.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct ConfigInfo {
    /// Resolved provider when the model is loaded, otherwise the requested one.
    pub provider: String,
    pub requested_provider: String,
    /// Effective batch size when the model is loaded, otherwise the requested one.
    pub batch_size: usize,
    pub requested_batch_size: usize,
    /// `dynamic` or `fixed:N`; `None` when the model is not loaded.
    pub model_batch: Option<String>,
    /// Requested CUDA tuning ([`CudaTuning::describe`](crate::inference_options::CudaTuning::describe)).
    pub cuda_tuning: String,
    /// Whether the tuning was passed to the CUDA EP; `None` when unknown.
    pub cuda_applied: Option<bool>,
}

impl ConfigInfo {
    pub fn new(requested: &InferenceOptions, effective: Option<&EffectiveInference>) -> Self {
        let requested_provider = requested.provider.as_str().to_string();
        let requested_batch_size = requested.batch_size.get();
        let cuda_tuning = requested.cuda.describe();
        match effective {
            Some(eff) => Self {
                provider: eff.provider.as_str().to_string(),
                requested_provider,
                batch_size: eff.batch_size,
                requested_batch_size,
                model_batch: Some(model_batch_label(eff.model_batch)),
                cuda_tuning,
                cuda_applied: Some(eff.cuda_applied),
            },
            None => Self {
                provider: requested_provider.clone(),
                requested_provider,
                batch_size: requested_batch_size,
                requested_batch_size,
                model_batch: None,
                cuda_tuning,
                cuda_applied: None,
            },
        }
    }
}

/// Stage timings (ms) and throughput of one analysis, or their sum over files.
#[derive(Debug, Clone, Default, PartialEq, Serialize)]
pub struct PerfReport {
    pub windows: usize,
    /// Only when the analysis itself loaded the model.
    pub model_load_ms: Option<f64>,
    pub preprocess_ms: f64,
    pub inference_ms: f64,
    pub postprocess_ms: f64,
    pub output_ms: f64,
    pub total_ms: f64,
    /// 0 when no time was recorded.
    pub windows_per_s_total: f64,
    /// 0 when no time was recorded.
    pub windows_per_s_inference: f64,
}

impl From<&AnalyzePerf> for PerfReport {
    fn from(perf: &AnalyzePerf) -> Self {
        let t = &perf.timings;
        Self {
            windows: perf.windows,
            model_load_ms: t.model_load.map(ms),
            preprocess_ms: ms(t.preprocess),
            inference_ms: ms(t.inference),
            postprocess_ms: ms(t.postprocess),
            output_ms: ms(t.output),
            total_ms: ms(t.total),
            windows_per_s_total: perf.windows_per_sec_total(),
            windows_per_s_inference: perf.windows_per_sec_inference(),
        }
    }
}

impl PerfReport {
    /// Times and windows summed; throughput recomputed from the sums.
    fn sum<'a>(items: impl IntoIterator<Item = &'a PerfReport>) -> Self {
        let mut total = Self::default();
        for p in items {
            total.windows += p.windows;
            total.model_load_ms = match (total.model_load_ms, p.model_load_ms) {
                (None, None) => None,
                (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
            };
            total.preprocess_ms += p.preprocess_ms;
            total.inference_ms += p.inference_ms;
            total.postprocess_ms += p.postprocess_ms;
            total.output_ms += p.output_ms;
            total.total_ms += p.total_ms;
        }
        total.windows_per_s_total = windows_per_sec(total.windows, total.total_ms);
        total.windows_per_s_inference = windows_per_sec(total.windows, total.inference_ms);
        total
    }
}

/// Observed data of one ECL: beat rows of both analyses, the window
/// accumulator filled during the candidate run, and both measurements.
pub struct FileObservation<'a> {
    pub ecl: &'a str,
    pub baseline_rows: &'a [BeatResultRow],
    pub candidate_rows: &'a [BeatResultRow],
    pub windows: &'a WindowAccumulator,
    pub baseline_perf: &'a AnalyzePerf,
    pub candidate_perf: &'a AnalyzePerf,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct FileReport {
    pub ecl: String,
    pub accuracy: AccuracyMetrics,
    pub baseline_perf: PerfReport,
    pub candidate_perf: PerfReport,
}

impl FileReport {
    pub fn from_observation(
        obs: &FileObservation<'_>,
        tolerance_samples: u32,
    ) -> Result<Self, MetricsError> {
        Ok(Self {
            ecl: obs.ecl.to_string(),
            accuracy: compute_accuracy(
                obs.baseline_rows,
                obs.candidate_rows,
                obs.windows,
                tolerance_samples,
            )?,
            baseline_perf: PerfReport::from(obs.baseline_perf),
            candidate_perf: PerfReport::from(obs.candidate_perf),
        })
    }
}

/// Cross-file summary. Speed-ups are candidate / baseline windows per
/// second, `None` when either side recorded no time.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct AggregateReport {
    pub files: usize,
    pub accuracy: AccuracyMetrics,
    pub baseline_perf: PerfReport,
    pub candidate_perf: PerfReport,
    pub speedup_total: Option<f64>,
    pub speedup_inference: Option<f64>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct CompareReport {
    pub baseline: ConfigInfo,
    pub candidate: ConfigInfo,
    pub tolerance_samples: u32,
    pub files: Vec<FileReport>,
    pub aggregate: AggregateReport,
    /// `None` when no threshold was given.
    pub verdict: Option<Verdict>,
}

impl CompareReport {
    /// [`EXIT_THRESHOLD_FAILED`] when a given threshold failed, otherwise
    /// [`EXIT_SUCCESS`].
    pub fn exit_code(&self) -> i32 {
        match &self.verdict {
            Some(v) if !v.passed => EXIT_THRESHOLD_FAILED,
            _ => EXIT_SUCCESS,
        }
    }
}

/// Aggregates per-file reports and judges the given thresholds.
pub fn assemble_report(
    baseline: ConfigInfo,
    candidate: ConfigInfo,
    tolerance_samples: u32,
    files: Vec<FileReport>,
    thresholds: &Thresholds,
) -> CompareReport {
    let accuracy: Vec<AccuracyMetrics> = files.iter().map(|f| f.accuracy.clone()).collect();
    let baseline_perf = PerfReport::sum(files.iter().map(|f| &f.baseline_perf));
    let candidate_perf = PerfReport::sum(files.iter().map(|f| &f.candidate_perf));
    let summary = AggregateReport {
        files: files.len(),
        accuracy: aggregate(&accuracy),
        speedup_total: speedup(
            baseline_perf.windows_per_s_total,
            candidate_perf.windows_per_s_total,
        ),
        speedup_inference: speedup(
            baseline_perf.windows_per_s_inference,
            candidate_perf.windows_per_s_inference,
        ),
        baseline_perf,
        candidate_perf,
    };
    let verdict = thresholds.evaluate(&summary.accuracy);
    CompareReport {
        baseline,
        candidate,
        tolerance_samples,
        files,
        aggregate: summary,
        verdict,
    }
}

/// Builds the whole report from observed data of every ECL.
pub fn build_report(
    baseline: ConfigInfo,
    candidate: ConfigInfo,
    tolerance_samples: u32,
    observations: &[FileObservation<'_>],
    thresholds: &Thresholds,
) -> Result<CompareReport, MetricsError> {
    let files = observations
        .iter()
        .map(|obs| FileReport::from_observation(obs, tolerance_samples))
        .collect::<Result<Vec<_>, _>>()?;
    Ok(assemble_report(
        baseline,
        candidate,
        tolerance_samples,
        files,
        thresholds,
    ))
}

/// Pretty-printed `report.json`; NaN values are written as `null`.
pub fn to_json(report: &CompareReport) -> String {
    serde_json::to_string_pretty(report).expect("CompareReport has only JSON-representable fields")
}

/// Human-readable `report.md`.
pub fn to_markdown(report: &CompareReport) -> String {
    let mut md = String::from("# 推論高速化 比較レポート\n\n");
    render_settings(&mut md, report);
    render_verdict(&mut md, report.verdict.as_ref());

    md.push_str("## 集計\n\n");
    md.push_str(&format!("対象ファイル数: {}\n\n", report.aggregate.files));
    render_accuracy(&mut md, "###", &report.aggregate.accuracy);
    render_perf(
        &mut md,
        "###",
        &report.aggregate.baseline_perf,
        &report.aggregate.candidate_perf,
    );

    md.push_str("## ファイル別\n\n");
    for (i, file) in report.files.iter().enumerate() {
        md.push_str(&format!("### {}. {}\n\n", i + 1, file.ecl));
        render_accuracy(&mut md, "####", &file.accuracy);
        render_perf(&mut md, "####", &file.baseline_perf, &file.candidate_perf);
    }
    md
}

/// Writes `report.json` and `report.md` into `dir` (created if missing,
/// existing files overwritten) and returns their paths.
pub fn write_report(report: &CompareReport, dir: &Path) -> io::Result<(PathBuf, PathBuf)> {
    fs::create_dir_all(dir)?;
    let json_path = dir.join(REPORT_JSON);
    fs::write(&json_path, to_json(report))?;
    let md_path = dir.join(REPORT_MARKDOWN);
    fs::write(&md_path, to_markdown(report))?;
    Ok((json_path, md_path))
}

fn render_settings(md: &mut String, report: &CompareReport) {
    let (b, c) = (&report.baseline, &report.candidate);
    md.push_str("## 設定\n\n| 項目 | 基準 | 候補 |\n|---|---|---|\n");
    table_row3(
        md,
        "実行プロバイダ",
        &with_requested(&b.provider, &b.requested_provider),
        &with_requested(&c.provider, &c.requested_provider),
    );
    table_row3(
        md,
        "まとめ件数",
        &with_requested(
            &b.batch_size.to_string(),
            &b.requested_batch_size.to_string(),
        ),
        &with_requested(
            &c.batch_size.to_string(),
            &c.requested_batch_size.to_string(),
        ),
    );
    table_row3(
        md,
        "モデルのバッチ形状",
        b.model_batch.as_deref().unwrap_or(ABSENT),
        c.model_batch.as_deref().unwrap_or(ABSENT),
    );
    table_row3(md, "CUDA チューニング", &tuning_cell(b), &tuning_cell(c));
    let tolerance_ms = f64::from(report.tolerance_samples) * 1000.0 / f64::from(FS);
    md.push_str(&format!(
        "\n拍の対応付け許容幅: {} サンプル（{} ms）\n\n",
        report.tolerance_samples, tolerance_ms
    ));
}

fn render_verdict(md: &mut String, verdict: Option<&Verdict>) {
    md.push_str("## 判定\n\n");
    let Some(v) = verdict else {
        md.push_str("閾値の指定なし（合否は判定していません）\n\n");
        return;
    };
    if v.passed {
        md.push_str("**合格**: 指定されたすべての閾値を満たしています\n\n");
    } else {
        md.push_str(&format!(
            "**不合格**: 指定された閾値を満たさない指標があります（終了コード {EXIT_THRESHOLD_FAILED}）\n\n"
        ));
    }
    md.push_str("| 閾値 | 実測（集計） | 条件 | 結果 |\n|---|---|---|---|\n");
    for check in &v.checks {
        let op = if check.metric.starts_with("min_") {
            "≥"
        } else {
            "≤"
        };
        md.push_str(&format!(
            "| {} | {:.6} | {op} {} | {} |\n",
            check.metric,
            check.actual,
            check.threshold,
            if check.passed { "合格" } else { "不合格" }
        ));
    }
    md.push('\n');
}

fn render_accuracy(md: &mut String, heading: &str, m: &AccuracyMetrics) {
    let matched = m.matched_beats > 0;
    let p = &m.prob_max_abs_diff;
    let sampled = p.sampled_windows > 0;
    md.push_str(&format!("{heading} 精度\n\n| 指標 | 値 |\n|---|---|\n"));
    let rows = [
        ("比較窓数", m.windows.to_string()),
        (
            "リズム区間の一致率（窓単位）",
            percent_or_none(m.rhythm_window_agreement, m.windows > 0),
        ),
        (
            "検出拍数 基準 / 候補",
            format!("{} / {}", m.baseline_beats, m.candidate_beats),
        ),
        (
            "検出拍数の差（候補 − 基準）",
            format!("{:+}", m.beat_count_diff),
        ),
        ("対応拍数", m.matched_beats.to_string()),
        (
            "対応率（基準比）",
            percent_or_none(m.match_rate_vs_baseline, m.baseline_beats > 0),
        ),
        (
            "対応率（候補比）",
            percent_or_none(m.match_rate_vs_candidate, m.candidate_beats > 0),
        ),
        (
            "位置ずれ 最大 / 平均 (サンプル)",
            if matched {
                format!("{:.1} / {:.1}", m.offset_max_samples, m.offset_mean_samples)
            } else {
                NO_COMPARISON.to_string()
            },
        ),
        (
            "拍ラベル一致率",
            percent_or_none(m.beat_class_agreement, matched),
        ),
        (
            "リズムラベル一致率",
            percent_or_none(m.rhythm_class_agreement, matched),
        ),
        (
            "Unknown 一致率",
            percent_or_none(m.unknown_agreement, matched),
        ),
        (
            "短連発フラグ一致率",
            percent_or_none(m.short_run_agreement, matched),
        ),
        (
            "出力確率を比較した窓数（beat / event）",
            p.sampled_windows.to_string(),
        ),
        ("出力確率の最大絶対差 beat", diff_or_none(p.beat, sampled)),
        (
            "出力確率の最大絶対差 event PAC",
            diff_or_none(p.event_pac, sampled),
        ),
        (
            "出力確率の最大絶対差 event PVC",
            diff_or_none(p.event_pvc, sampled),
        ),
        (
            "出力確率の最大絶対差 event N",
            diff_or_none(p.event_n, sampled),
        ),
        (
            "出力確率の最大絶対差 rhythm",
            diff_or_none(p.rhythm, m.windows > 0),
        ),
    ];
    for (label, value) in &rows {
        md.push_str(&format!("| {label} | {value} |\n"));
    }

    md.push_str(
        "\n拍ラベル混同表（行 = 基準、列 = 候補、対応拍のみ）\n\n\
         | 基準 \\ 候補 | N | PAC | PVC |\n|---|---|---|---|\n",
    );
    for (label, row) in ["N", "PAC", "PVC"].iter().zip(&m.beat_class_confusion) {
        md.push_str(&format!(
            "| {label} | {} | {} | {} |\n",
            row[0], row[1], row[2]
        ));
    }
    md.push('\n');
}

fn render_perf(md: &mut String, heading: &str, baseline: &PerfReport, candidate: &PerfReport) {
    md.push_str(&format!(
        "{heading} 速度\n\n| 段階 | 基準 | 候補 |\n|---|---|---|\n"
    ));
    let load = |p: &PerfReport| p.model_load_ms.map_or_else(|| ABSENT.to_string(), fmt_ms);
    table_row3(md, "モデル読込み (ms)", &load(baseline), &load(candidate));
    let (b, c) = (baseline, candidate);
    let stages = [
        ("前処理 (ms)", b.preprocess_ms, c.preprocess_ms),
        ("推論 (ms)", b.inference_ms, c.inference_ms),
        ("後処理 (ms)", b.postprocess_ms, c.postprocess_ms),
        ("出力生成 (ms)", b.output_ms, c.output_ms),
        ("総時間 (ms)", b.total_ms, c.total_ms),
    ];
    for (label, b_ms, c_ms) in stages {
        table_row3(md, label, &fmt_ms(b_ms), &fmt_ms(c_ms));
    }
    table_row3(
        md,
        "ウィンドウ数",
        &baseline.windows.to_string(),
        &candidate.windows.to_string(),
    );
    table_row3(
        md,
        "ウィンドウ/秒（総時間）",
        &format!("{:.1}", baseline.windows_per_s_total),
        &format!("{:.1}", candidate.windows_per_s_total),
    );
    table_row3(
        md,
        "ウィンドウ/秒（推論）",
        &format!("{:.1}", baseline.windows_per_s_inference),
        &format!("{:.1}", candidate.windows_per_s_inference),
    );

    let ratio = |s: Option<f64>| s.map_or_else(|| ABSENT.to_string(), |s| format!("{s:.2} 倍"));
    md.push_str("\n| 速度比 | 値 |\n|---|---|\n");
    md.push_str(&format!(
        "| 速度比（候補/基準・総時間） | {} |\n",
        ratio(speedup(
            baseline.windows_per_s_total,
            candidate.windows_per_s_total
        ))
    ));
    md.push_str(&format!(
        "| 速度比（候補/基準・推論） | {} |\n\n",
        ratio(speedup(
            baseline.windows_per_s_inference,
            candidate.windows_per_s_inference
        ))
    ));
}

fn table_row3(md: &mut String, label: &str, baseline: &str, candidate: &str) {
    md.push_str(&format!("| {label} | {baseline} | {candidate} |\n"));
}

fn with_requested(effective: &str, requested: &str) -> String {
    if effective == requested {
        effective.to_string()
    } else {
        format!("{effective}（要求: {requested}）")
    }
}

fn tuning_cell(config: &ConfigInfo) -> String {
    match config.cuda_applied {
        Some(false) => format!("{}（未適用）", config.cuda_tuning),
        _ => config.cuda_tuning.clone(),
    }
}

fn percent_or_none(rate: f64, comparable: bool) -> String {
    if comparable {
        format!("{:.2}%", rate * 100.0)
    } else {
        NO_COMPARISON.to_string()
    }
}

fn diff_or_none(diff: f64, comparable: bool) -> String {
    if comparable {
        format!("{diff:.6}")
    } else {
        NO_COMPARISON.to_string()
    }
}

fn speedup(baseline_wps: f64, candidate_wps: f64) -> Option<f64> {
    (baseline_wps > 0.0 && candidate_wps > 0.0).then(|| candidate_wps / baseline_wps)
}

fn windows_per_sec(windows: usize, elapsed_ms: f64) -> f64 {
    if elapsed_ms > 0.0 {
        windows as f64 * 1000.0 / elapsed_ms
    } else {
        0.0
    }
}

fn ms(d: Duration) -> f64 {
    d.as_secs_f64() * 1000.0
}

fn fmt_ms(value: f64) -> String {
    format!("{value:.1}")
}

fn model_batch_label(shape: ModelBatchShape) -> String {
    match shape {
        ModelBatchShape::Dynamic => "dynamic".to_string(),
        ModelBatchShape::Fixed(n) => format!("fixed:{n}"),
    }
}

fn nan_min(a: f64, b: f64) -> f64 {
    if a.is_nan() || b.is_nan() {
        f64::NAN
    } else {
        a.min(b)
    }
}

fn largest_prob_diff(p: &ProbDiff) -> f64 {
    [p.beat, p.event_pac, p.event_pvc, p.event_n, p.rhythm]
        .into_iter()
        .fold(0.0, |acc, v| if v.is_nan() || v > acc { v } else { acc })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference_options::{BatchSize, CudaTuning};
    use crate::perf::StageTimings;
    use crate::phase2::{ExecutionProviderKind, ModelBatchShape, WindowOutputs};
    use std::time::Duration;

    const EPS: f64 = 1e-6;

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < EPS,
            "expected {expected}, got {actual}"
        );
    }

    fn row(ms: i64, class: &str) -> BeatResultRow {
        let secs = ms / 1000;
        BeatResultRow {
            record_id: "rec".into(),
            beat_idx: 0,
            beat_time: format!(
                "2025-01-01 00:{:02}:{:02}.{:03}",
                secs / 60,
                secs % 60,
                ms % 1000
            ),
            unknown: 0,
            beat_class: class.into(),
            rhythm_class: "SR".into(),
            short_run_flag: 0,
        }
    }

    fn perf(total_ms: u64, inference_ms: u64, windows: usize) -> AnalyzePerf {
        AnalyzePerf {
            timings: StageTimings {
                model_load: None,
                preprocess: Duration::from_millis(100),
                inference: Duration::from_millis(inference_ms),
                postprocess: Duration::from_millis(50),
                output: Duration::from_millis(25),
                total: Duration::from_millis(total_ms),
            },
            windows,
            effective: None,
        }
    }

    fn outputs(beat: f32) -> WindowOutputs {
        WindowOutputs {
            beat: vec![beat],
            event: vec![[0.5, 0.5, 0.5]],
            rhythm: 0.0,
        }
    }

    fn baseline_config() -> ConfigInfo {
        let opts = InferenceOptions {
            provider: ExecutionProviderKind::Cpu,
            batch_size: BatchSize::new(1).unwrap(),
            cuda: CudaTuning::default(),
        };
        ConfigInfo::new(&opts, None)
    }

    fn candidate_config() -> ConfigInfo {
        let opts = InferenceOptions {
            provider: ExecutionProviderKind::Auto,
            batch_size: BatchSize::new(32).unwrap(),
            cuda: CudaTuning {
                tf32: None,
                conv1d_pad_to_nc1d: Some(true),
                cuda_graph: Some(false),
            },
        };
        let effective = EffectiveInference {
            provider: ExecutionProviderKind::Cuda,
            batch_size: 16,
            requested_batch_size: 32,
            model_batch: ModelBatchShape::Fixed(16),
            pad_tail: true,
            cuda: opts.cuda,
            cuda_applied: true,
            cuda_graph_active: false,
            notes: Vec::new(),
        };
        ConfigInfo::new(&opts, Some(&effective))
    }

    /// Observed data of two files:
    /// - a.ecl: 4 vs 3 beats, 3 matched (offsets 5, 0, 20 samples), labels
    ///   agree, 2 windows agree, beat prob diff 0.25; 2 windows each side.
    /// - b.ecl: 2 vs 2 beats, both matched, 1 label differs, 1 of 2 windows
    ///   agrees.
    struct Observed {
        a_base: Vec<BeatResultRow>,
        a_cand: Vec<BeatResultRow>,
        a_win: WindowAccumulator,
        a_base_perf: AnalyzePerf,
        a_cand_perf: AnalyzePerf,
        b_base: Vec<BeatResultRow>,
        b_cand: Vec<BeatResultRow>,
        b_win: WindowAccumulator,
        b_base_perf: AnalyzePerf,
        b_cand_perf: AnalyzePerf,
    }

    impl Observed {
        fn new() -> Self {
            let mut a_win = WindowAccumulator::default();
            a_win.observe_rhythm(0.1, 0.1);
            a_win.observe_rhythm(0.9, 0.9);
            a_win.observe_outputs(&outputs(0.5), &outputs(0.75));
            let mut b_win = WindowAccumulator::default();
            b_win.observe_rhythm(0.1, 0.9);
            b_win.observe_rhythm(0.2, 0.2);
            Self {
                a_base: vec![
                    row(1000, "N"),
                    row(2000, "PAC"),
                    row(3000, "PVC"),
                    row(4000, "N"),
                ],
                a_cand: vec![row(1010, "N"), row(2000, "PAC"), row(3040, "PVC")],
                a_win,
                a_base_perf: perf(2000, 1000, 2),
                a_cand_perf: perf(500, 250, 2),
                b_base: vec![row(1000, "N"), row(2000, "N")],
                b_cand: vec![row(1000, "N"), row(2000, "PVC")],
                b_win,
                b_base_perf: perf(2000, 1000, 2),
                b_cand_perf: perf(1000, 500, 2),
            }
        }

        fn observations(&self) -> Vec<FileObservation<'_>> {
            vec![
                FileObservation {
                    ecl: "a.ecl",
                    baseline_rows: &self.a_base,
                    candidate_rows: &self.a_cand,
                    windows: &self.a_win,
                    baseline_perf: &self.a_base_perf,
                    candidate_perf: &self.a_cand_perf,
                },
                FileObservation {
                    ecl: "b.ecl",
                    baseline_rows: &self.b_base,
                    candidate_rows: &self.b_cand,
                    windows: &self.b_win,
                    baseline_perf: &self.b_base_perf,
                    candidate_perf: &self.b_cand_perf,
                },
            ]
        }

        fn report(&self, thresholds: &Thresholds) -> CompareReport {
            build_report(
                baseline_config(),
                candidate_config(),
                40,
                &self.observations(),
                thresholds,
            )
            .unwrap()
        }
    }

    fn single_file_report(base: &[BeatResultRow], cand: &[BeatResultRow]) -> CompareReport {
        let windows = WindowAccumulator::default();
        let p = perf(1000, 500, 0);
        let obs = [FileObservation {
            ecl: "only.ecl",
            baseline_rows: base,
            candidate_rows: cand,
            windows: &windows,
            baseline_perf: &p,
            candidate_perf: &p,
        }];
        build_report(
            baseline_config(),
            candidate_config(),
            40,
            &obs,
            &Thresholds::default(),
        )
        .unwrap()
    }

    #[test]
    fn thresholds_have_no_default_values() {
        let t = Thresholds::default();
        assert_eq!(t.min_rhythm_window_agreement, None);
        assert_eq!(t.min_beat_match_rate, None);
        assert_eq!(t.min_beat_class_agreement, None);
        assert_eq!(t.max_prob_abs_diff, None);
        assert_eq!(t.max_offset_samples, None);
        assert!(t.is_empty());
        assert!(!Thresholds {
            max_offset_samples: Some(10.0),
            ..Thresholds::default()
        }
        .is_empty());
    }

    #[test]
    fn config_info_prefers_effective_settings_and_keeps_requested() {
        let base = baseline_config();
        assert_eq!(base.provider, "cpu");
        assert_eq!(base.requested_provider, "cpu");
        assert_eq!(base.batch_size, 1);
        assert_eq!(base.requested_batch_size, 1);
        assert_eq!(base.model_batch, None);
        assert_eq!(
            base.cuda_tuning,
            "tf32=default,conv1d_pad_to_nc1d=default,cuda_graph=default"
        );
        assert_eq!(base.cuda_applied, None);

        let cand = candidate_config();
        assert_eq!(cand.provider, "cuda");
        assert_eq!(cand.requested_provider, "auto");
        assert_eq!(cand.batch_size, 16);
        assert_eq!(cand.requested_batch_size, 32);
        assert_eq!(cand.model_batch.as_deref(), Some("fixed:16"));
        assert_eq!(
            cand.cuda_tuning,
            "tf32=default,conv1d_pad_to_nc1d=on,cuda_graph=off"
        );
        assert_eq!(cand.cuda_applied, Some(true));
    }

    #[test]
    fn perf_report_carries_stage_times_and_windows_per_second() {
        let mut p = perf(2000, 1000, 4);
        p.timings.model_load = Some(Duration::from_micros(812_300));
        let r = PerfReport::from(&p);
        assert_eq!(r.windows, 4);
        assert_close(r.model_load_ms.unwrap(), 812.3);
        assert_close(r.preprocess_ms, 100.0);
        assert_close(r.inference_ms, 1000.0);
        assert_close(r.postprocess_ms, 50.0);
        assert_close(r.output_ms, 25.0);
        assert_close(r.total_ms, 2000.0);
        assert_close(r.windows_per_s_total, 2.0);
        assert_close(r.windows_per_s_inference, 4.0);
        assert_eq!(PerfReport::from(&perf(2000, 1000, 4)).model_load_ms, None);
    }

    #[test]
    fn build_report_assembles_files_and_aggregate_from_observed_data() {
        let report = Observed::new().report(&Thresholds::default());
        assert_eq!(report.tolerance_samples, 40);
        assert_eq!(report.baseline, baseline_config());
        assert_eq!(report.candidate, candidate_config());
        assert_eq!(report.files.len(), 2);

        let a = &report.files[0];
        assert_eq!(a.ecl, "a.ecl");
        assert_eq!(a.accuracy.baseline_beats, 4);
        assert_eq!(a.accuracy.candidate_beats, 3);
        assert_eq!(a.accuracy.matched_beats, 3);
        assert_close(a.accuracy.offset_max_samples, 20.0);
        assert_close(a.accuracy.rhythm_window_agreement, 1.0);
        assert_close(a.accuracy.prob_max_abs_diff.beat, 0.25);
        assert_close(a.baseline_perf.windows_per_s_total, 1.0);
        assert_close(a.candidate_perf.windows_per_s_inference, 8.0);

        let b = &report.files[1];
        assert_eq!(b.ecl, "b.ecl");
        assert_eq!(b.accuracy.matched_beats, 2);
        assert_close(b.accuracy.beat_class_agreement, 0.5);
        assert_close(b.accuracy.rhythm_window_agreement, 0.5);

        let agg = &report.aggregate;
        assert_eq!(agg.files, 2);
        assert_eq!(agg.accuracy.windows, 4);
        assert_eq!(agg.accuracy.baseline_beats, 6);
        assert_eq!(agg.accuracy.candidate_beats, 5);
        assert_eq!(agg.accuracy.matched_beats, 5);
        assert_close(agg.accuracy.match_rate_vs_baseline, 5.0 / 6.0);
        assert_close(agg.accuracy.match_rate_vs_candidate, 1.0);
        assert_close(agg.accuracy.beat_class_agreement, 0.8);
        assert_close(agg.accuracy.rhythm_window_agreement, 0.75);

        assert_eq!(agg.baseline_perf.windows, 4);
        assert_close(agg.baseline_perf.total_ms, 4000.0);
        assert_close(agg.baseline_perf.inference_ms, 2000.0);
        assert_close(agg.baseline_perf.preprocess_ms, 200.0);
        assert_close(agg.baseline_perf.windows_per_s_total, 1.0);
        assert_close(agg.baseline_perf.windows_per_s_inference, 2.0);
        assert_eq!(agg.baseline_perf.model_load_ms, None);
        assert_close(agg.candidate_perf.total_ms, 1500.0);
        assert_close(agg.candidate_perf.windows_per_s_total, 4.0 / 1.5);
        assert_close(agg.candidate_perf.windows_per_s_inference, 4.0 / 0.75);
        assert_close(agg.speedup_total.unwrap(), 4.0 / 1.5);
        assert_close(agg.speedup_inference.unwrap(), (4.0 / 0.75) / 2.0);
    }

    #[test]
    fn assemble_report_matches_build_report() {
        let observed = Observed::new();
        let files = observed
            .observations()
            .iter()
            .map(|o| FileReport::from_observation(o, 40).unwrap())
            .collect();
        let assembled = assemble_report(
            baseline_config(),
            candidate_config(),
            40,
            files,
            &Thresholds::default(),
        );
        assert_eq!(assembled, observed.report(&Thresholds::default()));
    }

    #[test]
    fn build_report_propagates_unparseable_beat_time() {
        let mut bad = row(1000, "N");
        bad.beat_time = "garbage".into();
        let windows = WindowAccumulator::default();
        let p = perf(1, 1, 1);
        let obs = [FileObservation {
            ecl: "bad.ecl",
            baseline_rows: std::slice::from_ref(&bad),
            candidate_rows: &[],
            windows: &windows,
            baseline_perf: &p,
            candidate_perf: &p,
        }];
        let err = build_report(
            baseline_config(),
            candidate_config(),
            40,
            &obs,
            &Thresholds::default(),
        )
        .unwrap_err();
        assert!(matches!(
            err,
            MetricsError::BeatTime {
                side: "baseline",
                ..
            }
        ));
    }

    #[test]
    fn aggregate_speedup_is_none_without_recorded_time() {
        let report = single_file_report(&[], &[]);
        assert_eq!(report.aggregate.speedup_total, None);
        assert_eq!(report.aggregate.speedup_inference, None);
    }

    #[test]
    fn no_thresholds_means_no_verdict_and_exit_success() {
        let report = Observed::new().report(&Thresholds::default());
        assert_eq!(report.verdict, None);
        assert_eq!(report.exit_code(), EXIT_SUCCESS);
        let json: serde_json::Value = serde_json::from_str(&to_json(&report)).unwrap();
        assert!(json["verdict"].is_null());
        let md = to_markdown(&report);
        assert!(md.contains("閾値の指定なし"), "{md}");
        assert!(!md.contains("合格"), "{md}");
    }

    #[test]
    fn only_given_thresholds_are_checked_and_boundaries_pass() {
        let t = Thresholds {
            min_rhythm_window_agreement: Some(0.75),
            max_offset_samples: Some(20.0),
            ..Thresholds::default()
        };
        let report = Observed::new().report(&t);
        let v = report.verdict.as_ref().unwrap();
        assert!(v.passed);
        assert_eq!(v.checks.len(), 2);
        assert_eq!(v.checks[0].metric, "min_rhythm_window_agreement");
        assert_close(v.checks[0].actual, 0.75);
        assert_close(v.checks[0].threshold, 0.75);
        assert!(v.checks[0].passed);
        assert_eq!(v.checks[1].metric, "max_offset_samples");
        assert_close(v.checks[1].actual, 20.0);
        assert!(v.checks[1].passed);
        assert_eq!(report.exit_code(), EXIT_SUCCESS);
        assert!(to_markdown(&report).contains("合格"));
    }

    #[test]
    fn failing_threshold_fails_verdict_and_exit_code_is_two() {
        let t = Thresholds {
            min_rhythm_window_agreement: Some(0.5),
            min_beat_match_rate: Some(0.9),
            min_beat_class_agreement: Some(0.8),
            max_prob_abs_diff: Some(0.2),
            max_offset_samples: Some(25.0),
        };
        let report = Observed::new().report(&t);
        let v = report.verdict.as_ref().unwrap();
        assert!(!v.passed);
        let names: Vec<&str> = v.checks.iter().map(|c| c.metric.as_str()).collect();
        assert_eq!(
            names,
            [
                "min_rhythm_window_agreement",
                "min_beat_match_rate",
                "min_beat_class_agreement",
                "max_prob_abs_diff",
                "max_offset_samples",
            ]
        );
        let passed: Vec<bool> = v.checks.iter().map(|c| c.passed).collect();
        assert_eq!(passed, [true, false, true, false, true]);
        assert_close(v.checks[1].actual, 5.0 / 6.0);
        // Largest series is rhythm (b.ecl window 0.1 vs 0.9), not beat 0.25.
        assert_close(v.checks[3].actual, 0.8);
        assert_eq!(report.exit_code(), EXIT_THRESHOLD_FAILED);
        assert!(to_markdown(&report).contains("不合格"));
    }

    #[test]
    fn beat_match_rate_is_judged_on_lower_of_both_directions() {
        let mut m = Observed::new()
            .report(&Thresholds::default())
            .aggregate
            .accuracy;
        m.match_rate_vs_baseline = 1.0;
        m.match_rate_vs_candidate = 0.6;
        let t = Thresholds {
            min_beat_match_rate: Some(0.7),
            ..Thresholds::default()
        };
        let v = t.evaluate(&m).unwrap();
        assert_close(v.checks[0].actual, 0.6);
        assert!(!v.passed);
    }

    #[test]
    fn prob_abs_diff_is_judged_on_largest_series() {
        let mut m = Observed::new()
            .report(&Thresholds::default())
            .aggregate
            .accuracy;
        m.prob_max_abs_diff.event_pvc = 0.4;
        m.prob_max_abs_diff.rhythm = 0.3;
        let t = Thresholds {
            max_prob_abs_diff: Some(0.35),
            ..Thresholds::default()
        };
        let v = t.evaluate(&m).unwrap();
        assert_close(v.checks[0].actual, 0.4);
        assert!(!v.passed);
    }

    #[test]
    fn nan_metrics_fail_min_and_max_thresholds_and_serialize_as_null() {
        let mut report = Observed::new().report(&Thresholds::default());
        report.aggregate.accuracy.rhythm_window_agreement = f64::NAN;
        report.aggregate.accuracy.prob_max_abs_diff.rhythm = f64::NAN;
        let t = Thresholds {
            min_rhythm_window_agreement: Some(0.0),
            max_prob_abs_diff: Some(1.0),
            ..Thresholds::default()
        };
        let v = t.evaluate(&report.aggregate.accuracy).unwrap();
        assert!(v.checks.iter().all(|c| !c.passed && c.actual.is_nan()));
        assert!(!v.passed);
        report.verdict = Some(v);
        assert_eq!(report.exit_code(), EXIT_THRESHOLD_FAILED);
        let json: serde_json::Value = serde_json::from_str(&to_json(&report)).unwrap();
        assert!(json["verdict"]["checks"][0]["actual"].is_null());
        assert!(json["aggregate"]["accuracy"]["prob_max_abs_diff"]["rhythm"].is_null());
    }

    #[test]
    fn json_contains_required_keys() {
        let t = Thresholds {
            max_offset_samples: Some(25.0),
            ..Thresholds::default()
        };
        let json: serde_json::Value =
            serde_json::from_str(&to_json(&Observed::new().report(&t))).unwrap();
        for side in ["baseline", "candidate"] {
            for key in [
                "provider",
                "requested_provider",
                "batch_size",
                "requested_batch_size",
                "model_batch",
                "cuda_tuning",
                "cuda_applied",
            ] {
                assert!(json[side].get(key).is_some(), "missing {side}.{key}");
            }
        }
        assert_eq!(json["candidate"]["provider"], "cuda");
        assert_eq!(json["candidate"]["batch_size"], 16);
        assert_eq!(json["tolerance_samples"], 40);

        let perf_keys = [
            "windows",
            "model_load_ms",
            "preprocess_ms",
            "inference_ms",
            "postprocess_ms",
            "output_ms",
            "total_ms",
            "windows_per_s_total",
            "windows_per_s_inference",
        ];
        let files = json["files"].as_array().unwrap();
        assert_eq!(files.len(), 2);
        assert_eq!(files[0]["ecl"], "a.ecl");
        for file in files {
            assert!(file["accuracy"]["matched_beats"].is_u64());
            assert!(file["accuracy"]["beat_class_confusion"].is_array());
            assert!(file["accuracy"]["prob_max_abs_diff"]["beat"].is_number());
            for side in ["baseline_perf", "candidate_perf"] {
                for key in perf_keys {
                    assert!(file[side].get(key).is_some(), "missing {side}.{key}");
                }
            }
        }
        let agg = &json["aggregate"];
        assert_eq!(agg["files"], 2);
        assert!(agg["accuracy"]["match_rate_vs_baseline"].is_number());
        for key in ["speedup_total", "speedup_inference"] {
            assert!(agg.get(key).is_some(), "missing aggregate.{key}");
        }
        for side in ["baseline_perf", "candidate_perf"] {
            for key in perf_keys {
                assert!(
                    agg[side].get(key).is_some(),
                    "missing aggregate.{side}.{key}"
                );
            }
        }
        let check = &json["verdict"]["checks"][0];
        assert_eq!(check["metric"], "max_offset_samples");
        assert_eq!(check["threshold"], 25.0);
        assert_eq!(check["passed"], true);
        assert_eq!(json["verdict"]["passed"], true);
    }

    #[test]
    fn markdown_summarizes_settings_files_accuracy_and_speed() {
        let md = to_markdown(&Observed::new().report(&Thresholds::default()));
        for needle in [
            "# 推論高速化 比較レポート",
            "tf32=default,conv1d_pad_to_nc1d=on,cuda_graph=off",
            "cuda（要求: auto）",
            "16（要求: 32）",
            "fixed:16",
            "40 サンプル（80 ms）",
            "a.ecl",
            "b.ecl",
            "| 対応率（基準比） | 83.33% |",
            "| 拍ラベル一致率 | 80.00% |",
            "| 推論 (ms) | 2000.0 | 750.0 |",
            "| 総時間 (ms) | 4000.0 | 1500.0 |",
            "| ウィンドウ/秒（総時間） | 1.0 | 2.7 |",
            "| ウィンドウ/秒（推論） | 2.0 | 5.3 |",
            "| 速度比（候補/基準・推論） | 2.67 倍 |",
            "| N | 2 | 0 | 1 |",
        ] {
            assert!(md.contains(needle), "missing {needle:?} in\n{md}");
        }
    }

    #[test]
    fn markdown_shows_no_comparison_instead_of_vacuous_rates() {
        let md = to_markdown(&single_file_report(&[row(1000, "N")], &[row(9000, "N")]));
        assert!(md.contains("| 対応拍数 | 0 |"), "{md}");
        assert!(md.contains("| 対応率（基準比） | 0.00% |"), "{md}");
        for label in [
            "拍ラベル一致率",
            "リズムラベル一致率",
            "Unknown 一致率",
            "短連発フラグ一致率",
            "位置ずれ 最大 / 平均 (サンプル)",
            "リズム区間の一致率（窓単位）",
        ] {
            assert!(
                md.contains(&format!("| {label} | 比較対象なし |")),
                "{label} in\n{md}"
            );
        }
        assert!(!md.contains("100.00%"), "{md}");

        let empty = to_markdown(&single_file_report(&[], &[]));
        assert!(
            empty.contains("| 対応率（基準比） | 比較対象なし |"),
            "{empty}"
        );
        assert!(
            empty.contains("| 対応率（候補比） | 比較対象なし |"),
            "{empty}"
        );
    }

    #[test]
    fn write_report_writes_json_and_markdown_files() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("nested").join("report_dir");
        let report = Observed::new().report(&Thresholds::default());
        let (json_path, md_path) = write_report(&report, &out).unwrap();
        assert_eq!(json_path, out.join(REPORT_JSON));
        assert_eq!(md_path, out.join(REPORT_MARKDOWN));
        assert_eq!(
            std::fs::read_to_string(&json_path).unwrap(),
            to_json(&report)
        );
        assert_eq!(
            std::fs::read_to_string(&md_path).unwrap(),
            to_markdown(&report)
        );
    }
}
