//! CSV / JSON encoding of analyze results with CLI-equivalent field labels.
//!
//! Pure codec — route wiring belongs to later tasks.

use crate::analyze::{AnalyzeSummary, BeatResultRow};
use crate::http::error::HttpError;
use csv::WriterBuilder;
use serde::Serialize;

/// Encodes [`BeatResultRow`] / [`AnalyzeSummary`] for HTTP success responses.
pub struct ResponseCodec;

/// JSON success body: summary + rows (design AnalyzeHttpResponse / JSON 成功例).
#[derive(Debug, Serialize)]
pub struct AnalyzeJsonBody<'a> {
    pub summary: AnalyzeSummaryJson,
    pub rows: &'a [BeatResultRow],
}

/// Summary fields mirrored from [`AnalyzeSummary`] for stable JSON keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct AnalyzeSummaryJson {
    pub beats: usize,
    pub unknown_ones: usize,
    pub short_run_ones: usize,
    pub windows: usize,
}

impl From<&AnalyzeSummary> for AnalyzeSummaryJson {
    fn from(s: &AnalyzeSummary) -> Self {
        Self {
            beats: s.beats,
            unknown_ones: s.unknown_ones,
            short_run_ones: s.short_run_ones,
            windows: s.windows,
        }
    }
}

/// CLI `beat_results` CSV column order / labels (from `BeatResultRow` serde).
const CSV_HEADERS: &[&str] = &[
    "record_id",
    "beat_idx",
    "beat_time",
    "Unknown",
    "beat_class",
    "rhythm_class",
    "short_run_flag",
];

impl ResponseCodec {
    /// CSV with headers matching CLI `beat_results` (`BeatResultRow` serde labels).
    pub fn to_csv(rows: &[BeatResultRow]) -> Result<String, HttpError> {
        let mut wtr = WriterBuilder::new()
            .has_headers(true)
            .from_writer(Vec::new());
        if rows.is_empty() {
            // csv crate emits headers only on first serialize; empty must still match CLI.
            wtr.write_record(CSV_HEADERS)
                .map_err(|e| HttpError::internal(format!("csv header write failed: {e}")))?;
        } else {
            for row in rows {
                wtr.serialize(row)
                    .map_err(|e| HttpError::internal(format!("csv serialize failed: {e}")))?;
            }
        }
        wtr.flush()
            .map_err(|e| HttpError::internal(format!("csv flush failed: {e}")))?;
        let bytes = wtr
            .into_inner()
            .map_err(|e| HttpError::internal(format!("csv buffer failed: {e}")))?;
        String::from_utf8(bytes)
            .map_err(|e| HttpError::internal(format!("csv utf-8 failed: {e}")))
    }

    /// Machine-readable JSON with `summary` and `rows` (CLI-equivalent labels).
    pub fn to_json(
        rows: &[BeatResultRow],
        summary: &AnalyzeSummary,
    ) -> Result<String, HttpError> {
        let body = AnalyzeJsonBody {
            summary: AnalyzeSummaryJson::from(summary),
            rows,
        };
        serde_json::to_string(&body)
            .map_err(|e| HttpError::internal(format!("json serialize failed: {e}")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_row() -> BeatResultRow {
        BeatResultRow {
            record_id: "1234567890_20240101_0000_2359".into(),
            beat_idx: 0,
            beat_time: "00:00:01.000".into(),
            unknown: 0,
            beat_class: "N".into(),
            rhythm_class: "SR".into(),
            short_run_flag: 0,
        }
    }

    fn sample_summary() -> AnalyzeSummary {
        AnalyzeSummary {
            beats: 1,
            unknown_ones: 0,
            short_run_ones: 0,
            windows: 2,
        }
    }

    #[test]
    fn csv_headers_match_cli_beat_result_labels() {
        let csv = ResponseCodec::to_csv(&[sample_row()]).expect("csv");
        let header = csv.lines().next().expect("header line");
        for col in [
            "record_id",
            "beat_idx",
            "beat_time",
            "Unknown",
            "beat_class",
            "rhythm_class",
            "short_run_flag",
        ] {
            assert!(
                header.contains(col),
                "CSV header must include CLI label '{col}': {header}"
            );
        }
    }

    #[test]
    fn csv_row_values_preserve_labels() {
        let mut row = sample_row();
        row.unknown = 1;
        row.beat_class = "PVC".into();
        row.rhythm_class = "AF/AFL".into();
        row.short_run_flag = 0;
        let csv = ResponseCodec::to_csv(&[row]).expect("csv");
        let mut lines = csv.lines();
        let _header = lines.next();
        let data = lines.next().expect("data row");
        assert!(data.contains("PVC"), "beat_class: {data}");
        assert!(data.contains("AF/AFL"), "rhythm_class: {data}");
        // Unknown column value 1
        assert!(
            data.split(',').any(|c| c == "1"),
            "expected Unknown=1 in row: {data}"
        );
    }

    #[test]
    fn json_includes_summary_and_rows_with_cli_fields() {
        let row = sample_row();
        let summary = sample_summary();
        let json = ResponseCodec::to_json(&[row], &summary).expect("json");
        let v: serde_json::Value = serde_json::from_str(&json).expect("parse");
        assert_eq!(v["summary"]["beats"], 1);
        assert_eq!(v["summary"]["unknown_ones"], 0);
        assert_eq!(v["summary"]["short_run_ones"], 0);
        assert_eq!(v["summary"]["windows"], 2);
        let first = &v["rows"][0];
        assert_eq!(first["record_id"], "1234567890_20240101_0000_2359");
        assert_eq!(first["beat_idx"], 0);
        assert_eq!(first["beat_time"], "00:00:01.000");
        assert_eq!(first["Unknown"], 0);
        assert_eq!(first["beat_class"], "N");
        assert_eq!(first["rhythm_class"], "SR");
        assert_eq!(first["short_run_flag"], 0);
    }

    #[test]
    fn empty_rows_still_emit_csv_header_and_json_summary() {
        let summary = AnalyzeSummary {
            beats: 0,
            unknown_ones: 0,
            short_run_ones: 0,
            windows: 0,
        };
        let csv = ResponseCodec::to_csv(&[]).expect("empty csv");
        assert!(
            csv.contains("record_id") && csv.contains("Unknown"),
            "empty CSV must still have CLI headers: {csv}"
        );
        let json = ResponseCodec::to_json(&[], &summary).expect("empty json");
        let v: serde_json::Value = serde_json::from_str(&json).expect("parse");
        assert_eq!(v["summary"]["beats"], 0);
        assert_eq!(v["rows"].as_array().expect("rows").len(), 0);
    }
}
