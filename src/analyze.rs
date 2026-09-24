//! End-to-end ECL analysis pipeline (BeatSense `analyzer.py` Rust port).

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

pub fn analyze_ecl(
    ecl_path: &Path,
    onnx_path: &Path,
    output_csv: &Path,
) -> Result<(Vec<BeatResultRow>, AnalyzeSummary), AnalyzeError> {
    analyze_ecl_with_limit(ecl_path, onnx_path, output_csv, None, ExecutionProviderKind::Auto)
}

pub fn analyze_ecl_with_limit(
    ecl_path: &Path,
    onnx_path: &Path,
    output_csv: &Path,
    max_windows: Option<usize>,
    provider: ExecutionProviderKind,
) -> Result<(Vec<BeatResultRow>, AnalyzeSummary), AnalyzeError> {
    let source_info = parse_ecl_filename(ecl_path)?;
    let ecg_all_250 = read_ecl_adc_counts(ecl_path)?;
    eprintln!("[1/6] AI preprocessing ...");
    let signal = build_ai_continuous_signal(&ecg_all_250, &source_info)?;

    let mut starts = signal.starts_abs_500.clone();
    if let Some(limit) = max_windows {
        starts.truncate(limit);
    }

    eprintln!(
        "[2/6] ONNX inference: windows={} requested_provider={}",
        starts.len(),
        provider
    );
    let mut model = Phase2Model::load_with_provider(onnx_path, provider)?;
    eprintln!("[2/6] using execution provider={}", model.provider());
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
