//! ECL preprocessing — BeatSense `preprocess.py` Rust port.

use crate::dsp::{resample_poly_2x, sosfiltfilt, BPF_SOS_250};
use crate::phase2::WINDOW_SAMPLES;
use chrono::{Duration, NaiveDate, NaiveDateTime, NaiveTime};
use regex::Regex;
use std::fs::File;
use std::io::Read;
use std::path::Path;
use std::sync::OnceLock;
use thiserror::Error;

pub const ORIG_FS: u32 = 250;
pub const FS: u32 = 500;
pub const UPSAMPLE_FACTOR: usize = (FS / ORIG_FS) as usize;
pub const EXPECTED_24H_SAMPLES_250: usize = ORIG_FS as usize * 24 * 60 * 60;

pub const WINDOW_SEC: f32 = 20.0;
pub const OVERLAP_SEC: f32 = 3.0;
pub const STEP_SEC: f32 = 17.0;
pub const STEP_SAMPLES: usize = 8500; // 17s @ 500Hz
pub const WINDOW_CENTER: f32 = 4999.5; // (10000 - 1) / 2

const ECL_ZERO_LEVEL: i32 = 0x0800;
const ECL_ECG_HIGH4_MASK: u16 = 0xF000;
const ECL_ECG_LOW8_MASK: u16 = 0x00FF;

#[derive(Debug, Error)]
pub enum PreprocessError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    Invalid(String),
}

#[derive(Debug, Clone)]
pub struct EclSourceInfo {
    pub serial: String,
    pub study_date: NaiveDate,
    pub recording_start: NaiveDateTime,
    pub recording_end: NaiveDateTime,
    pub start_time_hhmm: String,
    pub end_time_hhmm: String,
}

fn filename_re() -> &'static Regex {
    static RE: OnceLock<Regex> = OnceLock::new();
    RE.get_or_init(|| {
        Regex::new(
            r"^(?P<serial>\d{10})_(?P<study_date>\d{8})_(?P<start_time>\d{4})_(?P<end_time>\d{4})\.ecl$",
        )
        .expect("ECL filename regex")
    })
}

pub fn parse_ecl_filename(path: &Path) -> Result<EclSourceInfo, PreprocessError> {
    let name = path
        .file_name()
        .and_then(|s| s.to_str())
        .ok_or_else(|| PreprocessError::Invalid("missing filename".into()))?;
    let caps = filename_re().captures(name).ok_or_else(|| {
        PreprocessError::Invalid(format!(
            "Unexpected ECL filename; expected \
             '[10-digit serial]_[yyyyMMdd]_[HHmm]_[HHmm].ecl': {name}"
        ))
    })?;

    let serial = caps["serial"].to_string();
    let study_date = NaiveDate::parse_from_str(&caps["study_date"], "%Y%m%d")
        .map_err(|e| PreprocessError::Invalid(e.to_string()))?;
    let start_hhmm = &caps["start_time"];
    let end_hhmm = &caps["end_time"];
    let start_t = NaiveTime::parse_from_str(start_hhmm, "%H%M")
        .map_err(|e| PreprocessError::Invalid(e.to_string()))?;
    let end_t = NaiveTime::parse_from_str(end_hhmm, "%H%M")
        .map_err(|e| PreprocessError::Invalid(e.to_string()))?;

    let recording_start = study_date.and_time(start_t);
    let mut recording_end = study_date.and_time(end_t);
    if end_hhmm == "2359" {
        recording_end = (study_date + Duration::days(1))
            .and_hms_milli_opt(0, 0, 0, 0)
            .unwrap()
            - Duration::milliseconds(1);
    } else if recording_end < recording_start {
        recording_end += Duration::days(1);
    }

    Ok(EclSourceInfo {
        serial,
        study_date,
        recording_start,
        recording_end,
        start_time_hhmm: start_hhmm.to_string(),
        end_time_hhmm: end_hhmm.to_string(),
    })
}

/// Decode ECL 16-bit LE words → zero-centered 12-bit ADC counts (float32).
pub fn read_ecl_adc_counts(path: &Path) -> Result<Vec<f32>, PreprocessError> {
    let mut file = File::open(path)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    if bytes.len() % 2 != 0 {
        return Err(PreprocessError::Invalid(
            "ECL size is not a multiple of 2 bytes".into(),
        ));
    }
    let n_words = bytes.len() / 2;
    if n_words < EXPECTED_24H_SAMPLES_250 {
        return Err(PreprocessError::Invalid(format!(
            "ECL shorter than 24 h: actual={n_words}, expected={EXPECTED_24H_SAMPLES_250}"
        )));
    }
    let words = &bytes[..EXPECTED_24H_SAMPLES_250 * 2];
    let mut ecg = Vec::with_capacity(EXPECTED_24H_SAMPLES_250);
    for chunk in words.chunks_exact(2) {
        let w = u16::from_le_bytes([chunk[0], chunk[1]]);
        let high4 = (w & ECL_ECG_HIGH4_MASK) >> 4;
        let low8 = w & ECL_ECG_LOW8_MASK;
        let raw12 = (high4 | low8) as i32;
        ecg.push((raw12 - ECL_ZERO_LEVEL) as f32);
    }
    Ok(ecg)
}

pub fn valid_range_250(
    info: &EclSourceInfo,
    n_samples: usize,
) -> Result<(usize, usize), PreprocessError> {
    let file_start = info.study_date.and_hms_opt(0, 0, 0).unwrap();
    let end_exclusive = info.recording_end + Duration::milliseconds(1);
    let start = ((info.recording_start - file_start).num_milliseconds() as f64 / 1000.0
        * ORIG_FS as f64)
        .round() as i64;
    let end = ((end_exclusive - file_start).num_milliseconds() as f64 / 1000.0 * ORIG_FS as f64)
        .ceil() as i64;
    let start = start.max(0) as usize;
    let end = (end as usize).min(n_samples);
    if end <= start {
        return Err(PreprocessError::Invalid(
            "Invalid valid recording range".into(),
        ));
    }
    Ok((start, end))
}

pub fn bandpass_filter_for_ai(x_250: &[f32]) -> Result<Vec<f32>, PreprocessError> {
    if x_250.len() < 32 {
        return Err(PreprocessError::Invalid(
            "ECG is too short for sosfiltfilt".into(),
        ));
    }
    Ok(sosfiltfilt(&BPF_SOS_250, x_250))
}

pub fn zscore_windows_batch(windows: &mut [Vec<f32>], eps: f32) {
    for w in windows.iter_mut() {
        crate::phase2::zscore_window_eps(w, eps);
    }
}

#[derive(Debug, Clone)]
pub struct AiContinuousSignal {
    pub ecg_valid_500: Vec<f32>,
    pub starts_abs_500: Vec<i64>,
    pub abs_valid_start_500: i64,
    pub abs_valid_end_500: i64,
}

pub fn build_ai_continuous_signal(
    ecg_all_250: &[f32],
    info: &EclSourceInfo,
) -> Result<AiContinuousSignal, PreprocessError> {
    let (valid_start_250, valid_end_250) = valid_range_250(info, ecg_all_250.len())?;
    let ecg_valid_250 = &ecg_all_250[valid_start_250..valid_end_250];
    let filtered = bandpass_filter_for_ai(ecg_valid_250)?;
    let ecg_valid_500 = resample_poly_2x(&filtered);
    let abs_valid_start_500 = (valid_start_250 * UPSAMPLE_FACTOR) as i64;
    let abs_valid_end_500 = abs_valid_start_500 + ecg_valid_500.len() as i64;
    if ecg_valid_500.len() < WINDOW_SAMPLES {
        return Err(PreprocessError::Invalid(format!(
            "Recording shorter than {WINDOW_SEC:.1} s"
        )));
    }
    let mut starts_local = Vec::new();
    let mut s = 0usize;
    while s + WINDOW_SAMPLES <= ecg_valid_500.len() {
        starts_local.push(s as i64);
        s += STEP_SAMPLES;
    }
    let starts_abs_500: Vec<i64> = starts_local
        .iter()
        .map(|s| abs_valid_start_500 + *s)
        .collect();
    Ok(AiContinuousSignal {
        ecg_valid_500,
        starts_abs_500,
        abs_valid_start_500,
        abs_valid_end_500,
    })
}

/// Materialize one z-scored window `(WINDOW_SAMPLES,)` from continuous 500 Hz ECG.
pub fn materialize_window(
    ecg_valid_500: &[f32],
    abs_valid_start_500: i64,
    start_abs_500: i64,
) -> Vec<f32> {
    let local = (start_abs_500 - abs_valid_start_500) as usize;
    let mut w = ecg_valid_500[local..local + WINDOW_SAMPLES].to_vec();
    crate::phase2::zscore_window(&mut w);
    w
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn parse_filename_2359() {
        let p = PathBuf::from("2501103675_20250512_1415_2359.ecl");
        let info = parse_ecl_filename(&p).unwrap();
        assert_eq!(info.serial, "2501103675");
        assert_eq!(
            info.study_date,
            NaiveDate::from_ymd_opt(2025, 5, 12).unwrap()
        );
        assert_eq!(
            info.recording_start,
            NaiveDate::from_ymd_opt(2025, 5, 12)
                .unwrap()
                .and_hms_opt(14, 15, 0)
                .unwrap()
        );
    }
}
