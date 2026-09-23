//! Holter ECG arrhythmia classification library.
//!
//! CLI is the first surface; the same types will back a future HTTP API.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;
use thiserror::Error;

/// Rhythm classes supported in v0.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum RhythmLabel {
    /// Sinus / normal rhythm.
    Normal,
    /// Atrial fibrillation.
    Af,
    /// Premature atrial contraction.
    Pac,
    /// Premature ventricular contraction.
    Pvc,
}

impl RhythmLabel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Normal => "NORMAL",
            Self::Af => "AF",
            Self::Pac => "PAC",
            Self::Pvc => "PVC",
        }
    }
}

impl std::fmt::Display for RhythmLabel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One classification result for an ECG input.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClassificationResult {
    pub label: RhythmLabel,
    /// Model confidence in \[0.0, 1.0\]. Stub always returns 0.0.
    pub confidence: f32,
    pub input_path: String,
    pub notes: String,
}

#[derive(Debug, Error)]
pub enum ClassifyError {
    #[error("input file not found: {0}")]
    NotFound(String),
    #[error("failed to read input: {0}")]
    Io(#[from] std::io::Error),
    #[error("input file is empty: {0}")]
    Empty(String),
}

/// Classify an ECG file path.
///
/// Current implementation is a **stub**: it validates the file exists and is
/// non-empty, then returns [`RhythmLabel::Normal`] with confidence 0.0.
/// Real inference will replace this body without changing the CLI/API shape.
pub fn classify_ecg(path: &Path) -> Result<ClassificationResult, ClassifyError> {
    if !path.exists() {
        return Err(ClassifyError::NotFound(path.display().to_string()));
    }

    let bytes = fs::read(path)?;
    if bytes.is_empty() {
        return Err(ClassifyError::Empty(path.display().to_string()));
    }

    Ok(ClassificationResult {
        label: RhythmLabel::Normal,
        confidence: 0.0,
        input_path: path.display().to_string(),
        notes: "stub classifier: inference not implemented yet".to_string(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;
    use tempfile::NamedTempFile;

    #[test]
    fn classify_rejects_missing_file() {
        let err = classify_ecg(Path::new("/tmp/does-not-exist-holter-assist.ecg")).unwrap_err();
        assert!(matches!(err, ClassifyError::NotFound(_)));
    }

    #[test]
    fn classify_stub_returns_normal_for_nonempty_file() {
        let mut file = NamedTempFile::new().unwrap();
        writeln!(file, "fake-ecg-bytes").unwrap();
        let result = classify_ecg(file.path()).unwrap();
        assert_eq!(result.label, RhythmLabel::Normal);
        assert_eq!(result.confidence, 0.0);
    }
}
