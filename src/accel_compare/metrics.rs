//! Accuracy metrics for the baseline vs candidate comparison (pure
//! functions): beat matching within a sample tolerance, rhythm / beat-class
//! agreement, label confusion matrix, maximum absolute output-probability
//! difference, and aggregation across files.

use crate::analyze::BeatResultRow;
use crate::phase2::{RhythmClass, WindowOutputs, EVENT_N, EVENT_PAC, EVENT_PVC};
use crate::preprocess::FS;
use chrono::NaiveDateTime;
use serde::{Deserialize, Serialize};
use thiserror::Error;

const MS_PER_SAMPLE: i64 = 1000 / FS as i64;
const BEAT_TIME_FORMAT: &str = "%Y-%m-%d %H:%M:%S%.f";
/// Row / column order of [`AccuracyMetrics::beat_class_confusion`].
const BEAT_CLASSES: [&str; 3] = ["N", "PAC", "PVC"];

#[derive(Debug, Error, PartialEq, Eq)]
pub enum MetricsError {
    #[error("invalid beat_time {value:?} in {side} row {index}")]
    BeatTime {
        side: &'static str,
        index: usize,
        value: String,
    },
}

/// Maximum absolute difference of model output probabilities between the
/// baseline and the candidate. A NaN difference sticks (reported as NaN).
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct ProbDiff {
    pub beat: f64,
    pub event_pac: f64,
    pub event_pvc: f64,
    pub event_n: f64,
    /// Over every observed window.
    pub rhythm: f64,
    /// Windows whose sample-level beat / event outputs were compared.
    pub sampled_windows: usize,
}

impl ProbDiff {
    fn merge(&mut self, other: &ProbDiff) {
        max_into(&mut self.beat, other.beat);
        max_into(&mut self.event_pac, other.event_pac);
        max_into(&mut self.event_pvc, other.event_pvc);
        max_into(&mut self.event_n, other.event_n);
        max_into(&mut self.rhythm, other.rhythm);
        self.sampled_windows += other.sampled_windows;
    }
}

/// Rates whose denominator is zero are 1.0 (nothing to disagree about);
/// offsets are in 500 Hz samples and 0.0 when no beat is matched.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct AccuracyMetrics {
    pub windows: usize,
    /// Share of windows whose SR / AF/AFL decision (`TH_AF`) agrees.
    pub rhythm_window_agreement: f64,
    pub baseline_beats: usize,
    pub candidate_beats: usize,
    /// `candidate_beats - baseline_beats`.
    pub beat_count_diff: i64,
    pub matched_beats: usize,
    pub match_rate_vs_baseline: f64,
    pub match_rate_vs_candidate: f64,
    pub offset_max_samples: f64,
    pub offset_mean_samples: f64,
    /// Rates below are over matched beats.
    pub beat_class_agreement: f64,
    /// Rows = baseline `[N, PAC, PVC]`, columns = candidate; other labels are
    /// left out of the matrix but still count in `beat_class_agreement`.
    pub beat_class_confusion: [[u64; 3]; 3],
    pub rhythm_class_agreement: f64,
    pub unknown_agreement: f64,
    pub short_run_agreement: f64,
    pub prob_max_abs_diff: ProbDiff,
}

/// Incremental per-window comparison, fed while the candidate is analyzed.
#[derive(Debug, Clone, Default)]
pub struct WindowAccumulator {
    windows: usize,
    rhythm_agree: usize,
    prob: ProbDiff,
}

impl WindowAccumulator {
    /// Counts one window: rhythm decision agreement and rhythm score diff.
    pub fn observe_rhythm(&mut self, baseline: f32, candidate: f32) {
        self.windows += 1;
        if RhythmClass::from_score(baseline) == RhythmClass::from_score(candidate) {
            self.rhythm_agree += 1;
        }
        max_into(&mut self.prob.rhythm, abs_diff(baseline, candidate));
    }

    /// Compares the sample-level beat / event outputs of one sampled window
    /// (does not count as a rhythm window).
    pub fn observe_outputs(&mut self, baseline: &WindowOutputs, candidate: &WindowOutputs) {
        self.prob.sampled_windows += 1;
        for (&b, &c) in baseline.beat.iter().zip(&candidate.beat) {
            max_into(&mut self.prob.beat, abs_diff(b, c));
        }
        for (b, c) in baseline.event.iter().zip(&candidate.event) {
            max_into(
                &mut self.prob.event_pac,
                abs_diff(b[EVENT_PAC], c[EVENT_PAC]),
            );
            max_into(
                &mut self.prob.event_pvc,
                abs_diff(b[EVENT_PVC], c[EVENT_PVC]),
            );
            max_into(&mut self.prob.event_n, abs_diff(b[EVENT_N], c[EVENT_N]));
        }
    }

    pub fn windows(&self) -> usize {
        self.windows
    }

    pub fn rhythm_window_agreement(&self) -> f64 {
        rate(self.rhythm_agree, self.windows)
    }

    pub fn prob_diff(&self) -> &ProbDiff {
        &self.prob
    }
}

/// Parses the analysis CSV `beat_time` (`YYYY-MM-DD HH:MM:SS.mmm`) into
/// milliseconds on a common time axis.
pub fn parse_beat_time_ms(beat_time: &str) -> Option<i64> {
    NaiveDateTime::parse_from_str(beat_time, BEAT_TIME_FORMAT)
        .ok()
        .map(|t| t.and_utc().timestamp_millis())
}

/// Matches beats one-to-one: in time order, each baseline beat takes the
/// nearest unmatched candidate within `tolerance_samples * 2 ms` (ties go to
/// the earlier candidate). Returns `(baseline_idx, candidate_idx,
/// candidate_ms - baseline_ms)` in baseline time order.
pub fn match_beats(
    baseline_ms: &[i64],
    candidate_ms: &[i64],
    tolerance_samples: u32,
) -> Vec<(usize, usize, i64)> {
    let tol_ms = i64::from(tolerance_samples) * MS_PER_SAMPLE;
    let baseline_order = time_order(baseline_ms);
    let candidate_order = time_order(candidate_ms);
    let candidate_sorted: Vec<i64> = candidate_order.iter().map(|&j| candidate_ms[j]).collect();
    let mut used = vec![false; candidate_sorted.len()];
    let mut pairs = Vec::new();

    for i in baseline_order {
        let t = baseline_ms[i];
        let start = candidate_sorted.partition_point(|&c| c < t - tol_ms);
        let mut best: Option<(usize, i64)> = None;
        for (pos, &c) in candidate_sorted.iter().enumerate().skip(start) {
            if c > t + tol_ms {
                break;
            }
            if used[pos] {
                continue;
            }
            let dt = c - t;
            match best {
                Some((_, best_dt)) if best_dt.abs() <= dt.abs() => {}
                _ => best = Some((pos, dt)),
            }
        }
        if let Some((pos, dt)) = best {
            used[pos] = true;
            pairs.push((i, candidate_order[pos], dt));
        }
    }
    pairs
}

/// Accuracy of one file: beat rows of both analyses plus the per-window
/// accumulator filled during the candidate run.
pub fn compute_accuracy(
    baseline: &[BeatResultRow],
    candidate: &[BeatResultRow],
    windows: &WindowAccumulator,
    tolerance_samples: u32,
) -> Result<AccuracyMetrics, MetricsError> {
    let baseline_ms = beat_times_ms(baseline, "baseline")?;
    let candidate_ms = beat_times_ms(candidate, "candidate")?;
    let pairs = match_beats(&baseline_ms, &candidate_ms, tolerance_samples);

    let mut counts = Counts {
        windows: windows.windows,
        rhythm_window_agree: windows.rhythm_agree,
        baseline_beats: baseline.len(),
        candidate_beats: candidate.len(),
        matched: pairs.len(),
        prob: windows.prob,
        ..Counts::default()
    };
    for &(i, j, dt_ms) in &pairs {
        let (b, c) = (&baseline[i], &candidate[j]);
        let offset = dt_ms.abs() as f64 / MS_PER_SAMPLE as f64;
        counts.offset_max = counts.offset_max.max(offset);
        counts.offset_sum += offset;
        counts.class_agree += usize::from(b.beat_class == c.beat_class);
        counts.rhythm_agree += usize::from(b.rhythm_class == c.rhythm_class);
        counts.unknown_agree += usize::from(b.unknown == c.unknown);
        counts.short_run_agree += usize::from(b.short_run_flag == c.short_run_flag);
        if let (Some(r), Some(k)) = (class_index(&b.beat_class), class_index(&c.beat_class)) {
            counts.confusion[r][k] += 1;
        }
    }
    Ok(counts.into_metrics())
}

/// Cross-file summary: counts summed, rates pooled over the summed
/// denominators, maxima maxed, means weighted by matched beats.
pub fn aggregate(files: &[AccuracyMetrics]) -> AccuracyMetrics {
    let mut total = Counts::default();
    for m in files {
        let matched = m.matched_beats;
        total.windows += m.windows;
        total.rhythm_window_agree += numerator(m.rhythm_window_agreement, m.windows);
        total.baseline_beats += m.baseline_beats;
        total.candidate_beats += m.candidate_beats;
        total.matched += matched;
        total.offset_max = total.offset_max.max(m.offset_max_samples);
        total.offset_sum += m.offset_mean_samples * matched as f64;
        total.class_agree += numerator(m.beat_class_agreement, matched);
        total.rhythm_agree += numerator(m.rhythm_class_agreement, matched);
        total.unknown_agree += numerator(m.unknown_agreement, matched);
        total.short_run_agree += numerator(m.short_run_agreement, matched);
        for (row, file_row) in total.confusion.iter_mut().zip(&m.beat_class_confusion) {
            for (cell, file_cell) in row.iter_mut().zip(file_row) {
                *cell += file_cell;
            }
        }
        total.prob.merge(&m.prob_max_abs_diff);
    }
    total.into_metrics()
}

/// Raw numerators / denominators shared by per-file and aggregate metrics.
#[derive(Default)]
struct Counts {
    windows: usize,
    rhythm_window_agree: usize,
    baseline_beats: usize,
    candidate_beats: usize,
    matched: usize,
    offset_max: f64,
    offset_sum: f64,
    class_agree: usize,
    confusion: [[u64; 3]; 3],
    rhythm_agree: usize,
    unknown_agree: usize,
    short_run_agree: usize,
    prob: ProbDiff,
}

impl Counts {
    fn into_metrics(self) -> AccuracyMetrics {
        let matched = self.matched;
        AccuracyMetrics {
            windows: self.windows,
            rhythm_window_agreement: rate(self.rhythm_window_agree, self.windows),
            baseline_beats: self.baseline_beats,
            candidate_beats: self.candidate_beats,
            beat_count_diff: self.candidate_beats as i64 - self.baseline_beats as i64,
            matched_beats: matched,
            match_rate_vs_baseline: rate(matched, self.baseline_beats),
            match_rate_vs_candidate: rate(matched, self.candidate_beats),
            offset_max_samples: self.offset_max,
            offset_mean_samples: if matched == 0 {
                0.0
            } else {
                self.offset_sum / matched as f64
            },
            beat_class_agreement: rate(self.class_agree, matched),
            beat_class_confusion: self.confusion,
            rhythm_class_agreement: rate(self.rhythm_agree, matched),
            unknown_agreement: rate(self.unknown_agree, matched),
            short_run_agreement: rate(self.short_run_agree, matched),
            prob_max_abs_diff: self.prob,
        }
    }
}

fn beat_times_ms(rows: &[BeatResultRow], side: &'static str) -> Result<Vec<i64>, MetricsError> {
    rows.iter()
        .enumerate()
        .map(|(index, r)| {
            parse_beat_time_ms(&r.beat_time).ok_or_else(|| MetricsError::BeatTime {
                side,
                index,
                value: r.beat_time.clone(),
            })
        })
        .collect()
}

/// Indices sorted by time (stable, so equal times keep input order).
fn time_order(ms: &[i64]) -> Vec<usize> {
    let mut order: Vec<usize> = (0..ms.len()).collect();
    order.sort_by_key(|&i| ms[i]);
    order
}

fn class_index(label: &str) -> Option<usize> {
    BEAT_CLASSES.iter().position(|&c| c == label)
}

fn rate(numerator: usize, denominator: usize) -> f64 {
    if denominator == 0 {
        1.0
    } else {
        numerator as f64 / denominator as f64
    }
}

/// Recovers the integer numerator of a rate produced by [`rate`].
fn numerator(rate: f64, denominator: usize) -> usize {
    if denominator == 0 {
        0
    } else {
        (rate * denominator as f64).round() as usize
    }
}

fn abs_diff(a: f32, b: f32) -> f64 {
    (f64::from(a) - f64::from(b)).abs()
}

fn max_into(acc: &mut f64, value: f64) {
    if value.is_nan() || value > *acc {
        *acc = value;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const EPS: f64 = 1e-6;

    fn assert_close(actual: f64, expected: f64) {
        assert!(
            (actual - expected).abs() < EPS,
            "expected {expected}, got {actual}"
        );
    }

    fn row(ms: i64, class: &str, rhythm: &str, unknown: i8, short_run: i8) -> BeatResultRow {
        let secs = ms / 1000;
        let frac = ms % 1000;
        BeatResultRow {
            record_id: "rec".into(),
            beat_idx: 0,
            beat_time: format!(
                "2025-01-01 00:{:02}:{:02}.{:03}",
                secs / 60,
                secs % 60,
                frac
            ),
            unknown,
            beat_class: class.into(),
            rhythm_class: rhythm.into(),
            short_run_flag: short_run,
        }
    }

    fn outputs(beat: Vec<f32>, event: Vec<[f32; 3]>, rhythm: f32) -> WindowOutputs {
        WindowOutputs {
            beat,
            event,
            rhythm,
        }
    }

    /// File A: 4 vs 4 beats, 3 matched (offsets 5, 5, 20 samples), 4 windows.
    fn file_a() -> AccuracyMetrics {
        let baseline = vec![
            row(1000, "N", "SR", 0, 0),
            row(2000, "PAC", "SR", 0, 1),
            row(3000, "PVC", "AF/AFL", 0, 0),
            row(4000, "N", "SR", 1, 0),
        ];
        let candidate = vec![
            row(1010, "N", "SR", 1, 0),
            row(1990, "PVC", "SR", 0, 1),
            row(3040, "PVC", "SR", 0, 1),
            row(9000, "N", "SR", 0, 0),
        ];
        let mut acc = WindowAccumulator::default();
        acc.observe_rhythm(0.1, 0.2);
        acc.observe_rhythm(0.9, 0.8);
        acc.observe_rhythm(0.86, 0.95);
        acc.observe_rhythm(0.5, 0.5);
        acc.observe_outputs(
            &outputs(
                vec![0.1, 0.5, 0.875],
                vec![[0.25, 0.5, 0.25], [0.0, 0.0, 1.0]],
                0.1,
            ),
            &outputs(
                vec![0.1, 0.25, 0.9375],
                vec![[0.5, 0.5, 0.125], [0.3, 0.0625, 1.0]],
                0.2,
            ),
        );
        compute_accuracy(&baseline, &candidate, &acc, 40).unwrap()
    }

    /// File B: 2 vs 1 beats, 1 matched (offset 30 samples), 2 windows.
    fn file_b() -> AccuracyMetrics {
        let baseline = vec![row(1000, "N", "SR", 0, 0), row(2000, "N", "SR", 0, 0)];
        let candidate = vec![row(1060, "N", "SR", 0, 0)];
        let mut acc = WindowAccumulator::default();
        acc.observe_rhythm(0.1, 0.1);
        acc.observe_rhythm(0.2, 0.9);
        compute_accuracy(&baseline, &candidate, &acc, 40).unwrap()
    }

    #[test]
    fn parse_beat_time_reads_analysis_csv_format_as_ms() {
        let t0 = parse_beat_time_ms("2025-01-01 00:00:00.000").unwrap();
        let t1 = parse_beat_time_ms("2025-01-01 00:00:01.502").unwrap();
        let t2 = parse_beat_time_ms("2025-01-02 00:00:00.000").unwrap();
        assert_eq!(t1 - t0, 1502);
        assert_eq!(t2 - t0, 86_400_000);
        assert_eq!(parse_beat_time_ms("not a time"), None);
        assert_eq!(parse_beat_time_ms(""), None);
    }

    #[test]
    fn match_beats_pairs_nearest_within_tolerance_one_to_one() {
        let baseline = [1000, 2000, 3000, 4000];
        let candidate = [1010, 2100, 2990, 3000, 5000];
        assert_eq!(
            match_beats(&baseline, &candidate, 40),
            vec![(0, 0, 10), (2, 3, 0)]
        );
    }

    #[test]
    fn match_beats_tolerance_boundary_is_inclusive() {
        // 40 samples at 500 Hz = 80 ms.
        assert_eq!(match_beats(&[1000], &[1080], 40), vec![(0, 0, 80)]);
        assert_eq!(match_beats(&[1000], &[920], 40), vec![(0, 0, -80)]);
        assert!(match_beats(&[1000], &[1082], 40).is_empty());
        assert_eq!(match_beats(&[1000], &[1010], 5), vec![(0, 0, 10)]);
        assert!(match_beats(&[1000], &[1012], 5).is_empty());
    }

    #[test]
    fn match_beats_does_not_reuse_a_candidate() {
        assert_eq!(match_beats(&[1000, 1020], &[1010], 40), vec![(0, 0, 10)]);
    }

    #[test]
    fn match_beats_breaks_distance_ties_toward_earlier_candidate() {
        assert_eq!(match_beats(&[1000], &[990, 1010], 40), vec![(0, 0, -10)]);
        assert_eq!(match_beats(&[1000], &[1010, 990], 40), vec![(0, 1, -10)]);
    }

    #[test]
    fn match_beats_handles_unsorted_input_in_time_order() {
        assert_eq!(
            match_beats(&[3000, 1000], &[1005, 2995], 40),
            vec![(1, 0, 5), (0, 1, -5)]
        );
    }

    #[test]
    fn match_beats_empty_inputs() {
        assert!(match_beats(&[], &[1000], 40).is_empty());
        assert!(match_beats(&[1000], &[], 40).is_empty());
    }

    #[test]
    fn window_accumulator_tracks_rhythm_agreement_and_max_abs_diff() {
        let mut acc = WindowAccumulator::default();
        acc.observe_rhythm(0.1, 0.2);
        acc.observe_rhythm(0.9, 0.8);
        acc.observe_rhythm(0.86, 0.95);
        acc.observe_rhythm(0.5, 0.5);
        assert_eq!(acc.windows(), 4);
        assert_close(acc.rhythm_window_agreement(), 0.75);
        assert_close(acc.prob_diff().rhythm, 0.1);
        assert_eq!(acc.prob_diff().sampled_windows, 0);
        assert_eq!(acc.prob_diff().beat, 0.0);

        acc.observe_outputs(
            &outputs(vec![0.5, 0.5], vec![[0.5, 0.5, 0.5]; 2], 0.0),
            &outputs(
                vec![0.75, 0.5],
                vec![[0.5, 0.25, 0.625], [1.0, 0.5, 0.5]],
                0.0,
            ),
        );
        acc.observe_outputs(
            &outputs(vec![0.0, 0.0], vec![[0.0; 3]; 2], 0.0),
            &outputs(vec![0.125, 0.0], vec![[0.25, 0.0, 0.0]; 2], 0.0),
        );
        let p = acc.prob_diff();
        assert_eq!(p.sampled_windows, 2);
        assert_close(p.beat, 0.25);
        assert_close(p.event_pac, 0.5);
        assert_close(p.event_pvc, 0.25);
        assert_close(p.event_n, 0.125);
        // Beat/event observations do not count as rhythm windows.
        assert_eq!(acc.windows(), 4);
        assert_close(p.rhythm, 0.1);
    }

    #[test]
    fn window_accumulator_propagates_nan_differences() {
        let mut acc = WindowAccumulator::default();
        acc.observe_outputs(
            &outputs(vec![0.5, 0.5], vec![[0.5; 3]; 2], 0.0),
            &outputs(vec![f32::NAN, 0.9], vec![[0.5; 3]; 2], 0.0),
        );
        acc.observe_rhythm(0.1, f32::NAN);
        acc.observe_rhythm(0.1, 0.2);
        assert!(acc.prob_diff().beat.is_nan());
        assert!(acc.prob_diff().rhythm.is_nan());
        assert_close(acc.prob_diff().event_pac, 0.0);
    }

    #[test]
    fn compute_accuracy_reports_expected_metrics_for_known_beats() {
        let m = file_a();
        assert_eq!(m.windows, 4);
        assert_close(m.rhythm_window_agreement, 0.75);
        assert_eq!(m.baseline_beats, 4);
        assert_eq!(m.candidate_beats, 4);
        assert_eq!(m.beat_count_diff, 0);
        assert_eq!(m.matched_beats, 3);
        assert_close(m.match_rate_vs_baseline, 0.75);
        assert_close(m.match_rate_vs_candidate, 0.75);
        assert_close(m.offset_max_samples, 20.0);
        assert_close(m.offset_mean_samples, 10.0);
        assert_close(m.beat_class_agreement, 2.0 / 3.0);
        assert_eq!(m.beat_class_confusion, [[1, 0, 0], [0, 0, 1], [0, 0, 1]]);
        assert_close(m.rhythm_class_agreement, 2.0 / 3.0);
        assert_close(m.unknown_agreement, 2.0 / 3.0);
        assert_close(m.short_run_agreement, 2.0 / 3.0);
        let p = m.prob_max_abs_diff;
        assert_eq!(p.sampled_windows, 1);
        assert_close(p.beat, 0.25);
        assert_close(p.event_pac, 0.3);
        assert_close(p.event_pvc, 0.0625);
        assert_close(p.event_n, 0.125);
        assert_close(p.rhythm, 0.1);
    }

    #[test]
    fn compute_accuracy_counts_beat_difference_and_unmatched() {
        let m = file_b();
        assert_eq!(m.baseline_beats, 2);
        assert_eq!(m.candidate_beats, 1);
        assert_eq!(m.beat_count_diff, -1);
        assert_eq!(m.matched_beats, 1);
        assert_close(m.match_rate_vs_baseline, 0.5);
        assert_close(m.match_rate_vs_candidate, 1.0);
        assert_close(m.offset_max_samples, 30.0);
        assert_close(m.offset_mean_samples, 30.0);
        assert_close(m.beat_class_agreement, 1.0);
        assert_eq!(m.beat_class_confusion, [[1, 0, 0], [0, 0, 0], [0, 0, 0]]);
        assert_close(m.rhythm_window_agreement, 0.5);
        assert_close(m.prob_max_abs_diff.rhythm, 0.7);
    }

    #[test]
    fn compute_accuracy_excludes_unlisted_labels_from_confusion_only() {
        let baseline = vec![row(1000, "X", "SR", 0, 0), row(2000, "N", "SR", 0, 0)];
        let candidate = vec![row(1000, "X", "SR", 0, 0), row(2000, "PAC", "SR", 0, 0)];
        let m = compute_accuracy(&baseline, &candidate, &WindowAccumulator::default(), 40).unwrap();
        assert_close(m.beat_class_agreement, 0.5);
        assert_eq!(m.beat_class_confusion, [[0, 1, 0], [0, 0, 0], [0, 0, 0]]);
    }

    #[test]
    fn compute_accuracy_with_no_beats_or_windows_is_vacuously_in_agreement() {
        let m = compute_accuracy(&[], &[], &WindowAccumulator::default(), 40).unwrap();
        assert_eq!(m.windows, 0);
        assert_eq!(m.matched_beats, 0);
        assert_eq!(m.beat_count_diff, 0);
        for rate in [
            m.rhythm_window_agreement,
            m.match_rate_vs_baseline,
            m.match_rate_vs_candidate,
            m.beat_class_agreement,
            m.rhythm_class_agreement,
            m.unknown_agreement,
            m.short_run_agreement,
        ] {
            assert_close(rate, 1.0);
        }
        assert_eq!(m.offset_max_samples, 0.0);
        assert_eq!(m.offset_mean_samples, 0.0);
        assert_eq!(m.prob_max_abs_diff, ProbDiff::default());
    }

    #[test]
    fn compute_accuracy_rejects_unparseable_beat_time() {
        let mut bad = row(1000, "N", "SR", 0, 0);
        bad.beat_time = "garbage".into();
        let err = compute_accuracy(
            &[row(1000, "N", "SR", 0, 0)],
            &[row(1000, "N", "SR", 0, 0), bad],
            &WindowAccumulator::default(),
            40,
        )
        .unwrap_err();
        assert_eq!(
            err,
            MetricsError::BeatTime {
                side: "candidate",
                index: 1,
                value: "garbage".into()
            }
        );
    }

    #[test]
    fn aggregate_sums_counts_pools_rates_maxes_maxima_and_weights_means() {
        let agg = aggregate(&[file_a(), file_b()]);
        assert_eq!(agg.windows, 6);
        assert_close(agg.rhythm_window_agreement, 4.0 / 6.0);
        assert_eq!(agg.baseline_beats, 6);
        assert_eq!(agg.candidate_beats, 5);
        assert_eq!(agg.beat_count_diff, -1);
        assert_eq!(agg.matched_beats, 4);
        assert_close(agg.match_rate_vs_baseline, 4.0 / 6.0);
        assert_close(agg.match_rate_vs_candidate, 4.0 / 5.0);
        assert_close(agg.offset_max_samples, 30.0);
        assert_close(agg.offset_mean_samples, (10.0 * 3.0 + 30.0) / 4.0);
        assert_close(agg.beat_class_agreement, 3.0 / 4.0);
        assert_eq!(agg.beat_class_confusion, [[2, 0, 0], [0, 0, 1], [0, 0, 1]]);
        assert_close(agg.rhythm_class_agreement, 3.0 / 4.0);
        assert_close(agg.unknown_agreement, 3.0 / 4.0);
        assert_close(agg.short_run_agreement, 3.0 / 4.0);
        let p = agg.prob_max_abs_diff;
        assert_eq!(p.sampled_windows, 1);
        assert_close(p.beat, 0.25);
        assert_close(p.event_pac, 0.3);
        assert_close(p.event_pvc, 0.0625);
        assert_close(p.event_n, 0.125);
        assert_close(p.rhythm, 0.7);
    }

    #[test]
    fn aggregate_of_single_file_equals_that_file() {
        let a = file_a();
        let agg = aggregate(std::slice::from_ref(&a));
        assert_eq!(agg.windows, a.windows);
        assert_eq!(agg.matched_beats, a.matched_beats);
        assert_eq!(agg.beat_class_confusion, a.beat_class_confusion);
        assert_close(agg.beat_class_agreement, a.beat_class_agreement);
        assert_close(agg.offset_mean_samples, a.offset_mean_samples);
        assert_close(agg.rhythm_window_agreement, a.rhythm_window_agreement);
        assert_eq!(agg.prob_max_abs_diff, a.prob_max_abs_diff);
    }

    #[test]
    fn aggregate_of_no_files_matches_empty_comparison() {
        let empty = compute_accuracy(&[], &[], &WindowAccumulator::default(), 40).unwrap();
        assert_eq!(aggregate(&[]), empty);
    }

    #[test]
    fn accuracy_metrics_serializes_report_json_keys() {
        let v = serde_json::to_value(file_a()).unwrap();
        for key in [
            "windows",
            "rhythm_window_agreement",
            "baseline_beats",
            "candidate_beats",
            "beat_count_diff",
            "matched_beats",
            "match_rate_vs_baseline",
            "match_rate_vs_candidate",
            "offset_max_samples",
            "offset_mean_samples",
            "beat_class_agreement",
            "beat_class_confusion",
            "rhythm_class_agreement",
            "unknown_agreement",
            "short_run_agreement",
            "prob_max_abs_diff",
        ] {
            assert!(v.get(key).is_some(), "missing key {key}");
        }
        assert_eq!(v["beat_class_confusion"][1][2], 1);
        for key in [
            "beat",
            "event_pac",
            "event_pvc",
            "event_n",
            "rhythm",
            "sampled_windows",
        ] {
            assert!(
                v["prob_max_abs_diff"].get(key).is_some(),
                "missing prob key {key}"
            );
        }
    }
}
