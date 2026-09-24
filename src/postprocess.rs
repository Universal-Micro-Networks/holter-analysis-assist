//! Overlap-aware post-processing — BeatSense `postprocess.py` Rust port.

use crate::dsp::{find_peaks, resample_poly_2x};
use crate::phase2::{TH_AF, TH_BEAT, TH_PAC, TH_PVC};
use crate::preprocess::{
    valid_range_250, EclSourceInfo, FS, ORIG_FS, UPSAMPLE_FACTOR, WINDOW_CENTER,
};
use rustfft::num_complex::Complex;
use rustfft::FftPlanner;
use std::collections::HashSet;

pub const MIN_PEAK_DISTANCE_MS: f32 = 120.0;
pub const MIN_PEAK_DISTANCE: usize = 60; // round(500 * 120 / 1000)
pub const QRS_CLUSTER_TOL_MS: f32 = 80.0;
pub const QRS_CLUSTER_TOL: i64 = 40; // round(500 * 80 / 1000)
pub const EVENT_VECTOR_SEARCH_MS: f32 = 40.0;
pub const EVENT_VECTOR_SEARCH: usize = 20; // round(500 * 40 / 1000)

pub const RUN_SHORT_MIN_BEATS: usize = 4;
pub const RUN_LONG_MIN_BEATS: usize = 30;
pub const RUN_SHORT_RRI_RATIO_4_TO_29: f64 = 0.85;
pub const RUN_SHORT_RRI_RATIO_30_PLUS: f64 = 0.90;
pub const RUN_MIN_HR_BPM_4_TO_29: f64 = 100.0;
pub const RUN_MIN_HR_BPM_30_PLUS: f64 = 90.0;
pub const RUN_REFERENCE_RRI_COUNT: usize = 3;
pub const RUN_SUPPRESS_IN_AF: bool = true;

pub const UNKNOWN_AMPLITUDE_RATIO_THRESHOLD: f64 = 0.20;
pub const UNKNOWN_INTERVAL_MARGIN_SEC: f64 = 10.0;
pub const UNKNOWN_ABSOLUTE_AREA_EPS: f64 = 1e-12;
pub const UNKNOWN_LEGACY_WIN_SEC: f64 = 10.0;
pub const UNKNOWN_LEGACY_OVERLAP_SEC: f64 = 3.0;
pub const UNKNOWN_LEGACY_STEP_SEC: f64 = 7.0;
pub const UNKNOWN_LEGACY_SEG_LEN_250: usize = 2500; // 10s @ 250Hz
pub const UNKNOWN_LEGACY_STEP_LEN_250: usize = 1750; // 7s @ 250Hz
pub const UNKNOWN_LEGACY_SEG_LEN_500: usize = 5000; // 10s @ 500Hz

#[derive(Debug, Clone)]
pub struct BeatCandidate {
    pub abs_pos: i64,
    pub local_pos: usize,
    pub window_index: usize,
    pub beat_score: f32,
    pub center_distance: f32,
    pub pac_score: f32,
    pub pvc_score: f32,
    pub n_score: f32,
}

#[derive(Debug, Clone)]
pub struct DetectedBeat {
    pub sample_in_file_500: i64,
    pub beat_score: f32,
    pub pac_score: f32,
    pub pvc_score: f32,
    pub n_score: f32,
}

#[derive(Debug, Clone)]
pub struct RhythmWindow {
    pub start_sample_500: i64,
    pub end_sample_500: i64,
    pub rhythm_score: f32,
}

#[derive(Debug, Clone)]
pub struct RhythmInterval {
    pub start: i64,
    pub end: i64,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct RunCandidate {
    pub candidate_run_id: i64,
    pub start_sample_in_file_500: i64,
    pub end_sample_in_file_500: i64,
    pub member_endpoint_samples: Vec<i64>,
}

fn same_time_event_vector(
    event_window: &[[f32; 3]],
    local_position: usize,
    search_radius: usize,
) -> Option<([f32; 3], usize)> {
    let lo = local_position.saturating_sub(search_radius);
    let hi = (local_position + search_radius + 1).min(event_window.len());
    if hi <= lo {
        return None;
    }
    let mut best_i = lo;
    let mut best_max = f32::NEG_INFINITY;
    for (i, row) in event_window.iter().enumerate().take(hi).skip(lo) {
        let m = row[0].max(row[1]).max(row[2]);
        if m > best_max {
            best_max = m;
            best_i = i;
        }
    }
    let v = event_window[best_i];
    if v.iter().all(|x| x.is_finite()) {
        Some((v, best_i))
    } else {
        None
    }
}

/// Extract beat candidates from one window's beat/event predictions.
pub fn extract_window_candidates(
    beat: &[f32],
    event: &[[f32; 3]],
    start_abs_500: i64,
    global_window_index: usize,
) -> Vec<BeatCandidate> {
    let peaks = find_peaks(beat, TH_BEAT, MIN_PEAK_DISTANCE);
    let mut out = Vec::new();
    for (local_pos, beat_score) in peaks {
        if let Some((vec, _anchor)) = same_time_event_vector(event, local_pos, EVENT_VECTOR_SEARCH)
        {
            out.push(BeatCandidate {
                abs_pos: start_abs_500 + local_pos as i64,
                local_pos,
                window_index: global_window_index,
                beat_score,
                center_distance: (local_pos as f32 - WINDOW_CENTER).abs(),
                pac_score: vec[0],
                pvc_score: vec[1],
                n_score: vec[2],
            });
        }
    }
    out
}

pub fn cluster_candidates(mut candidates: Vec<BeatCandidate>) -> Vec<Vec<BeatCandidate>> {
    if candidates.is_empty() {
        return Vec::new();
    }
    candidates.sort_by_key(|c| c.abs_pos);
    let mut clusters = Vec::new();
    let mut current = vec![candidates[0].clone()];
    for c in candidates.into_iter().skip(1) {
        let mut positions: Vec<i64> = current.iter().map(|x| x.abs_pos).collect();
        positions.sort_unstable();
        let center = median_i64(&positions);
        if (c.abs_pos - center).abs() <= QRS_CLUSTER_TOL {
            current.push(c);
        } else {
            clusters.push(current);
            current = vec![c];
        }
    }
    clusters.push(current);
    clusters
}

fn median_i64(sorted: &[i64]) -> i64 {
    let n = sorted.len();
    if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2
    }
}

pub fn center_best_beats(clusters: Vec<Vec<BeatCandidate>>) -> Vec<DetectedBeat> {
    let mut beats = Vec::new();
    for cluster in clusters {
        let qrs_best = cluster
            .iter()
            .max_by(|a, b| {
                a.beat_score
                    .partial_cmp(&b.beat_score)
                    .unwrap_or(std::cmp::Ordering::Equal)
            })
            .unwrap();
        let class_best = cluster
            .iter()
            .min_by(|a, b| {
                a.center_distance
                    .partial_cmp(&b.center_distance)
                    .unwrap_or(std::cmp::Ordering::Equal)
                    .then_with(|| {
                        b.beat_score
                            .partial_cmp(&a.beat_score)
                            .unwrap_or(std::cmp::Ordering::Equal)
                    })
            })
            .unwrap();
        beats.push(DetectedBeat {
            sample_in_file_500: qrs_best.abs_pos,
            beat_score: qrs_best.beat_score,
            pac_score: class_best.pac_score,
            pvc_score: class_best.pvc_score,
            n_score: class_best.n_score,
        });
    }
    beats.sort_by_key(|b| b.sample_in_file_500);
    beats
}

pub fn classify_event_sigmoid(pac: f32, pvc: f32) -> &'static str {
    let pac_pos = pac >= TH_PAC;
    let pvc_pos = pvc >= TH_PVC;
    match (pac_pos, pvc_pos) {
        (true, true) => {
            if pac >= pvc {
                "PAC"
            } else {
                "PVC"
            }
        }
        (true, false) => "PAC",
        (false, true) => "PVC",
        (false, false) => "N",
    }
}

fn center_ownership_intervals(windows: &[RhythmWindow]) -> Vec<(usize, i64, i64)> {
    if windows.is_empty() {
        return Vec::new();
    }
    let starts: Vec<i64> = windows.iter().map(|w| w.start_sample_500).collect();
    let ends: Vec<i64> = windows.iter().map(|w| w.end_sample_500).collect();
    let centers: Vec<f64> = starts
        .iter()
        .zip(ends.iter())
        .map(|(&s, &e)| (s as f64 + e as f64 - 1.0) / 2.0)
        .collect();
    let mut out = Vec::new();
    for i in 0..windows.len() {
        let ws = starts[i];
        let we = ends[i];
        let mut os = if i == 0 {
            ws
        } else {
            ((centers[i - 1] + centers[i]) / 2.0).floor() as i64 + 1
        };
        let mut oe = if i + 1 == windows.len() {
            we
        } else {
            ((centers[i] + centers[i + 1]) / 2.0).floor() as i64 + 1
        };
        os = os.max(ws);
        oe = oe.min(we);
        if oe > os {
            out.push((i, os, oe));
        }
    }
    out
}

pub fn merge_intervals(intervals: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let mut items: Vec<(i64, i64)> = intervals.iter().copied().filter(|(a, b)| b > a).collect();
    items.sort_unstable();
    if items.is_empty() {
        return Vec::new();
    }
    let mut merged = vec![items[0]];
    for (a, b) in items.into_iter().skip(1) {
        let last = merged.last_mut().unwrap();
        if a <= last.1 {
            last.1 = last.1.max(b);
        } else {
            merged.push((a, b));
        }
    }
    merged
}

pub fn build_rhythm_intervals(
    mut windows: Vec<RhythmWindow>,
) -> (Vec<RhythmInterval>, Vec<(i64, i64)>) {
    windows.sort_by_key(|w| (w.start_sample_500, w.end_sample_500));
    let mut owned = Vec::new();
    for (i, os, oe) in center_ownership_intervals(&windows) {
        let score = windows[i].rhythm_score;
        let label = if score >= TH_AF { "AF/AFL" } else { "SR" };
        owned.push(RhythmInterval {
            start: os,
            end: oe,
            label: label.to_string(),
        });
    }
    let mut merged_rhythm: Vec<RhythmInterval> = Vec::new();
    for item in owned {
        if let Some(last) = merged_rhythm.last_mut() {
            if last.label == item.label && last.end == item.start {
                last.end = item.end;
                continue;
            }
        }
        merged_rhythm.push(item);
    }
    let coverage = merge_intervals(
        &merged_rhythm
            .iter()
            .map(|r| (r.start, r.end))
            .collect::<Vec<_>>(),
    );
    // coverage should be from owned before merge of labels - Python uses `owned` before label merge
    // Actually Python: coverage = merge_intervals((x["start"], x["end"]) for x in owned)
    // where owned is before merging same labels... wait, owned is list before merged_rhythm loop.
    // Looking again - owned has separate intervals, then merged_rhythm merges labels,
    // coverage uses owned (pre-label-merge). For contiguous ownership they're the same coverage.
    (merged_rhythm, coverage)
}

pub fn label_points_by_rhythm(points: &[i64], intervals: &[RhythmInterval]) -> Vec<String> {
    let mut labels = vec!["UNCOVERED".to_string(); points.len()];
    if points.is_empty() || intervals.is_empty() {
        return labels;
    }
    let starts: Vec<i64> = intervals.iter().map(|i| i.start).collect();
    for (j, &p) in points.iter().enumerate() {
        let i = match starts.binary_search(&p) {
            Ok(i) => i as isize,
            Err(i) => i as isize - 1,
        };
        if i >= 0 {
            let item = &intervals[i as usize];
            if item.start <= p && p < item.end {
                labels[j] = item.label.clone();
            }
        }
    }
    labels
}

pub fn point_in_intervals(points: &[i64], intervals: &[(i64, i64)]) -> Vec<bool> {
    points
        .iter()
        .map(|&p| intervals.iter().any(|&(a, b)| p >= a && p < b))
        .collect()
}

pub fn complement_intervals(start: i64, end: i64, covered: &[(i64, i64)]) -> Vec<(i64, i64)> {
    let clipped: Vec<(i64, i64)> = covered
        .iter()
        .filter_map(|&(a, b)| {
            let aa = a.max(start);
            let bb = b.min(end);
            if bb > start && aa < end && bb > aa {
                Some((aa, bb))
            } else {
                None
            }
        })
        .collect();
    let covered = merge_intervals(&clipped);
    let mut gaps = Vec::new();
    let mut cur = start;
    for (a, b) in covered {
        if a > cur {
            gaps.push((cur, a));
        }
        cur = cur.max(b);
    }
    if cur < end {
        gaps.push((cur, end));
    }
    gaps
}

fn make_interval_index(intervals: &[(i64, i64)]) -> (Vec<(i64, i64)>, Vec<i64>) {
    let merged = merge_intervals(intervals);
    let starts = merged.iter().map(|(a, _)| *a).collect();
    (merged, starts)
}

fn interval_overlaps(index: &(Vec<(i64, i64)>, Vec<i64>), start_abs: i64, end_abs: i64) -> bool {
    let (intervals, starts) = index;
    if intervals.is_empty() || end_abs <= start_abs {
        return false;
    }
    let i = match starts.binary_search(&start_abs) {
        Ok(i) => i as isize,
        Err(i) => i as isize - 1,
    };
    if i >= 0 && intervals[i as usize].1 > start_abs {
        return true;
    }
    let j = (i + 1) as usize;
    j < intervals.len() && intervals[j].0 < end_abs
}

/// Detect RUN candidates; returns accepted member endpoint sample positions for short_run_flag.
pub fn detect_run_candidates(
    beats: &[DetectedBeat],
    beat_classes: &[String],
    af_intervals: &[(i64, i64)],
    gap_intervals: &[(i64, i64)],
) -> (Vec<RunCandidate>, HashSet<i64>) {
    let _ = beat_classes;
    let pos: Vec<i64> = beats.iter().map(|b| b.sample_in_file_500).collect();
    if pos.len() < 2 {
        return (Vec::new(), HashSet::new());
    }
    let rr: Vec<f64> = pos
        .windows(2)
        .map(|w| (w[1] - w[0]) as f64 / FS as f64)
        .collect();
    let n_rr = rr.len();
    let af_index = make_interval_index(af_intervals);
    let gap_index = make_interval_index(gap_intervals);

    let mut rr_usable = vec![false; n_rr];
    for k in 0..n_rr {
        let a = pos[k];
        let b = pos[k + 1] + 1;
        let in_gap = interval_overlaps(&gap_index, a, b);
        let in_af = RUN_SUPPRESS_IN_AF && interval_overlaps(&af_index, a, b);
        if in_gap || in_af {
            continue;
        }
        let value = rr[k];
        if value.is_finite() && value > 0.0 {
            rr_usable[k] = true;
        }
    }

    let direct_previous3 = |i: usize| -> Option<(Vec<usize>, Vec<f64>, f64)> {
        if i < RUN_REFERENCE_RRI_COUNT {
            return None;
        }
        let ref_idx: Vec<usize> = ((i - RUN_REFERENCE_RRI_COUNT)..i).collect();
        if ref_idx.iter().any(|&k| !rr_usable[k]) {
            return None;
        }
        let ref_values: Vec<f64> = ref_idx.iter().map(|&k| rr[k]).collect();
        if ref_values.iter().any(|v| !(v.is_finite() && *v > 0.0)) {
            return None;
        }
        let mut sorted = ref_values.clone();
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let med = sorted[1]; // n=3
        if !(med.is_finite() && med > 0.0) {
            return None;
        }
        Some((ref_idx, ref_values, med))
    };

    let grow = |start: usize, threshold: f64| -> (Vec<usize>, Vec<f64>, usize) {
        let mut members = Vec::new();
        let mut values = Vec::new();
        let mut j = start;
        while j < n_rr {
            if !rr_usable[j] {
                break;
            }
            let cur = rr[j];
            if !(cur.is_finite() && cur > 0.0 && cur <= threshold) {
                break;
            }
            members.push(j);
            values.push(cur);
            j += 1;
        }
        (members, values, j)
    };

    let mut events: Vec<(Vec<usize>, f64, &'static str)> = Vec::new();
    let mut i_rr = RUN_REFERENCE_RRI_COUNT;
    while i_rr < n_rr {
        if !rr_usable[i_rr] {
            i_rr += 1;
            continue;
        }
        let Some((_ref_idx, _ref_values, reference_median)) = direct_previous3(i_rr) else {
            i_rr += 1;
            continue;
        };
        let strict_threshold = RUN_SHORT_RRI_RATIO_4_TO_29 * reference_median;
        let relaxed_threshold = RUN_SHORT_RRI_RATIO_30_PLUS * reference_median;

        let (relaxed_members, relaxed_values, relaxed_end) = grow(i_rr, relaxed_threshold);
        if relaxed_members.len() >= RUN_LONG_MIN_BEATS {
            let mean_rr = relaxed_values.iter().sum::<f64>() / relaxed_values.len() as f64;
            let hr = 60.0 / mean_rr;
            if hr.is_finite() && hr >= RUN_MIN_HR_BPM_30_PLUS {
                events.push((relaxed_members, reference_median, "30_plus"));
                i_rr = relaxed_end;
                continue;
            }
        }

        let (strict_members, strict_values, strict_end) = grow(i_rr, strict_threshold);
        if (RUN_SHORT_MIN_BEATS..RUN_LONG_MIN_BEATS).contains(&strict_members.len()) {
            let mean_rr = strict_values.iter().sum::<f64>() / strict_values.len() as f64;
            let hr = 60.0 / mean_rr;
            if hr.is_finite() && hr >= RUN_MIN_HR_BPM_4_TO_29 {
                events.push((strict_members, reference_median, "4_to_29"));
                i_rr = strict_end;
                continue;
            }
        }
        i_rr += 1;
    }

    let mut candidates = Vec::new();
    let mut member_samples = HashSet::new();
    for (rid, (member_rr_idx, _med, _cls)) in events.into_iter().enumerate() {
        let member_beat_idx: Vec<usize> = member_rr_idx.iter().map(|k| k + 1).collect();
        let start_abs = pos[member_beat_idx[0]];
        let end_abs = pos[*member_beat_idx.last().unwrap()];
        let endpoints: Vec<i64> = member_beat_idx.iter().map(|&i| pos[i]).collect();
        for &s in &endpoints {
            member_samples.insert(s);
        }
        candidates.push(RunCandidate {
            candidate_run_id: (rid + 1) as i64,
            start_sample_in_file_500: start_abs,
            end_sample_in_file_500: end_abs,
            member_endpoint_samples: endpoints,
        });
    }
    (candidates, member_samples)
}

pub fn finalize_runs_after_unknown(
    candidates: Vec<RunCandidate>,
    unknown_intervals: &[(i64, i64)],
) -> HashSet<i64> {
    let unknown_index = make_interval_index(unknown_intervals);
    let mut accepted = HashSet::new();
    for c in candidates {
        let overlaps = interval_overlaps(
            &unknown_index,
            c.start_sample_in_file_500,
            c.end_sample_in_file_500 + 1,
        );
        if !overlaps {
            for s in c.member_endpoint_samples {
                accepted.insert(s);
            }
        }
    }
    accepted
}

fn calc_amplitude_area_0_40(x: &[f64], fs: f64) -> f64 {
    let n = x.len();
    if n < 2 {
        return f64::NAN;
    }
    let mean = x.iter().sum::<f64>() / n as f64;
    let centered: Vec<f64> = x
        .iter()
        .map(|v| {
            let v = if v.is_finite() { *v } else { 0.0 };
            v - mean
        })
        .collect();

    // Hanning window
    let mut window = vec![0.0; n];
    for (i, w) in window.iter_mut().enumerate() {
        *w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n as f64 - 1.0)).cos();
    }
    let coherent_gain = {
        let m = window.iter().sum::<f64>() / n as f64;
        if m.is_finite() && m > 0.0 {
            m
        } else {
            1.0
        }
    };
    let mut buf: Vec<Complex<f64>> = centered
        .iter()
        .zip(window.iter())
        .map(|(x, w)| Complex::new(x * w, 0.0))
        .collect();
    // zero-pad to next power of two for rfft-like path via full FFT then take half
    let mut planner = FftPlanner::new();
    let fft = planner.plan_fft_forward(n);
    fft.process(&mut buf);

    let mut amp = Vec::with_capacity(n / 2 + 1);
    for (k, bin) in buf.iter().enumerate().take(n / 2 + 1) {
        let _ = k;
        amp.push(bin.norm() / (n as f64 * coherent_gain));
    }
    if n % 2 == 0 {
        if amp.len() > 2 {
            let last = amp.len() - 1;
            for a in amp.iter_mut().take(last).skip(1) {
                *a *= 2.0;
            }
        }
    } else if amp.len() > 1 {
        for a in amp.iter_mut().skip(1) {
            *a *= 2.0;
        }
    }

    let mut area = 0.0;
    let df = fs / n as f64;
    // trapezoid over 0 < f <= 40
    let mut prev_f = 0.0;
    let mut prev_a = 0.0;
    let mut have_prev = false;
    for (k, &a) in amp.iter().enumerate() {
        let f = k as f64 * df;
        if f > 0.0 && f <= 40.0 {
            let aa = if a.is_finite() { a } else { 0.0 };
            if have_prev {
                area += (aa + prev_a) * 0.5 * (f - prev_f);
            }
            prev_f = f;
            prev_a = aa;
            have_prev = true;
        }
    }
    area
}

fn legacy_zscore_and_restore(x_500: &[f32]) -> Vec<f64> {
    let n = x_500.len() as f32;
    let mean = x_500.iter().sum::<f32>() / n;
    let var = x_500.iter().map(|x| (x - mean) * (x - mean)).sum::<f32>() / n;
    let std = var.sqrt();
    let std = if std < 1e-6 { 1.0 } else { std };
    x_500
        .iter()
        .map(|&x| {
            let z = (x - mean) / std;
            z as f64 * std as f64 + mean as f64
        })
        .collect()
}

#[allow(clippy::type_complexity)]
pub fn build_unknown_intervals(
    ecg_all_250: &[f32],
    info: &EclSourceInfo,
    timeline_start_abs_500: i64,
    timeline_end_abs_500: i64,
) -> Result<(Vec<(i64, i64)>, f64, f64), String> {
    let (valid_start_250, valid_end_250) =
        valid_range_250(info, ecg_all_250.len()).map_err(|e| e.to_string())?;
    let file_start = info.study_date.and_hms_opt(0, 0, 0).unwrap();
    let recording_end_exclusive = info.recording_end + chrono::Duration::milliseconds(1);

    let mut rows: Vec<(i64, i64, f64)> = Vec::new();
    for hour_index in 0..24 {
        let hour_start = file_start + chrono::Duration::hours(hour_index);
        let hour_end = hour_start + chrono::Duration::hours(1);
        let data_start = hour_start.max(info.recording_start);
        let data_end = hour_end.min(recording_end_exclusive);
        if data_end <= data_start {
            continue;
        }
        let sample_start = ((data_start - file_start).num_milliseconds() as f64 / 1000.0
            * ORIG_FS as f64)
            .round() as i64;
        let sample_end = ((data_end - file_start).num_milliseconds() as f64 / 1000.0
            * ORIG_FS as f64)
            .ceil() as i64;
        let sample_start = (sample_start.max(0) as usize).max(valid_start_250);
        let sample_end = (sample_end as usize).min(valid_end_250);
        if sample_end.saturating_sub(sample_start) < UNKNOWN_LEGACY_SEG_LEN_250 {
            continue;
        }
        let ecg_hour = &ecg_all_250[sample_start..sample_end];
        let mut starts_local = Vec::new();
        let mut s = 0usize;
        while s + UNKNOWN_LEGACY_SEG_LEN_250 <= ecg_hour.len() {
            starts_local.push(s);
            s += UNKNOWN_LEGACY_STEP_LEN_250;
        }
        for s in starts_local {
            let seg = &ecg_hour[s..s + UNKNOWN_LEGACY_SEG_LEN_250];
            let mut x_500 = resample_poly_2x(seg);
            if x_500.len() > UNKNOWN_LEGACY_SEG_LEN_500 {
                x_500.truncate(UNKNOWN_LEGACY_SEG_LEN_500);
            } else if x_500.len() < UNKNOWN_LEGACY_SEG_LEN_500 {
                let pad = *x_500.last().unwrap_or(&0.0);
                x_500.resize(UNKNOWN_LEGACY_SEG_LEN_500, pad);
            }
            let before = legacy_zscore_and_restore(&x_500);
            let area = calc_amplitude_area_0_40(&before, FS as f64);
            let abs_start_500 = ((sample_start + s) * UPSAMPLE_FACTOR) as i64;
            let abs_end_500 = abs_start_500 + UNKNOWN_LEGACY_SEG_LEN_500 as i64;
            rows.push((abs_start_500, abs_end_500, area));
        }
    }
    if rows.is_empty() {
        return Err("No Unknown-QC windows could be built".into());
    }
    let mut finite: Vec<f64> = rows.iter().map(|r| r.2).filter(|a| a.is_finite()).collect();
    if finite.is_empty() {
        return Err("No finite Unknown-QC amplitude values".into());
    }
    finite.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median_area = if finite.len() % 2 == 1 {
        finite[finite.len() / 2]
    } else {
        0.5 * (finite[finite.len() / 2 - 1] + finite[finite.len() / 2])
    };
    if !(median_area.is_finite() && median_area > 0.0) {
        return Err(format!("Invalid Unknown reference median: {median_area}"));
    }
    let threshold =
        (median_area * UNKNOWN_AMPLITUDE_RATIO_THRESHOLD).max(UNKNOWN_ABSOLUTE_AREA_EPS);
    let core: Vec<(i64, i64)> = rows
        .iter()
        .filter(|(_, _, area)| !area.is_finite() || *area < threshold)
        .map(|(a, b, _)| (*a, *b))
        .collect();
    let core = merge_intervals(&core);
    let margin = (UNKNOWN_INTERVAL_MARGIN_SEC * FS as f64).round() as i64;
    let mut expanded = Vec::new();
    for (a, b) in core {
        let aa = (a - margin).max(timeline_start_abs_500);
        let bb = (b + margin).min(timeline_end_abs_500);
        if bb > aa {
            expanded.push((aa, bb));
        }
    }
    Ok((merge_intervals(&expanded), median_area, threshold))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_event_thresholds() {
        assert_eq!(classify_event_sigmoid(0.9, 0.2), "PAC");
        assert_eq!(classify_event_sigmoid(0.2, 0.9), "PVC");
        assert_eq!(classify_event_sigmoid(0.2, 0.2), "N");
        assert_eq!(classify_event_sigmoid(0.9, 0.95), "PVC");
    }

    #[test]
    fn merge_and_complement() {
        let m = merge_intervals(&[(0, 10), (8, 15), (20, 25)]);
        assert_eq!(m, vec![(0, 15), (20, 25)]);
        let gaps = complement_intervals(0, 30, &m);
        assert_eq!(gaps, vec![(15, 20), (25, 30)]);
    }
}
