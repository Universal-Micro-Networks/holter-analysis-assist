//! Task 5.1 consolidation: ModelSource / Phase2ModelLoader resolution & failure matrix.
//!
//! Individual cases already live in `model_source` and `phase2` unit tests (tasks 2.1/2.2).
//! This module asserts the acceptance matrix in one place so validation stays auditable.

use crate::model_source::{ModelSource, ModelSourceError};
use crate::phase2::{ExecutionProviderKind, InferError, Phase2Model};
use std::path::PathBuf;

/// Documented cells for requirements 1.2, 3.2, 3.3 (task 5.1).
const MATRIX_CELLS: &[&str] = &[
    "path_source_supported",
    "embedded_feature_gate",
    "empty_bytes_rejected",
    "missing_path_rejected",
    "embedded_session_optional",
];

#[test]
fn matrix_documents_required_cells() {
    assert_eq!(
        MATRIX_CELLS.len(),
        5,
        "task 5.1 matrix must keep five documented cells"
    );
    assert!(MATRIX_CELLS.contains(&"path_source_supported"));
    assert!(MATRIX_CELLS.contains(&"embedded_feature_gate"));
    assert!(MATRIX_CELLS.contains(&"empty_bytes_rejected"));
    assert!(MATRIX_CELLS.contains(&"missing_path_rejected"));
    assert!(MATRIX_CELLS.contains(&"embedded_session_optional"));
}

#[test]
fn path_source_supported() {
    let src = ModelSource::Path(PathBuf::from("resources/models/phase2_rev1.onnx"));
    assert!(src.ensure_supported().is_ok());
    assert!(matches!(src, ModelSource::Path(_)));
}

#[test]
fn embedded_feature_gate() {
    let src = ModelSource::Embedded;
    if cfg!(feature = "embedded-model") {
        assert!(src.ensure_supported().is_ok());
    } else {
        let err = src.ensure_supported().unwrap_err();
        assert!(matches!(err, ModelSourceError::EmbeddedUnavailable));
        let load_err = match Phase2Model::load_from_source(&src, ExecutionProviderKind::Cpu) {
            Err(e) => e,
            Ok(_) => panic!("Embedded without feature must fail"),
        };
        let msg = load_err.to_string();
        assert!(
            msg.contains("embedded-model") || msg.contains("Embedded"),
            "Embedded without feature must fail clearly: {msg}"
        );
    }
}

#[test]
fn empty_bytes_rejected() {
    let err = match Phase2Model::load_from_memory(&[], ExecutionProviderKind::Cpu) {
        Err(e) => e,
        Ok(_) => panic!("empty bytes must fail before inference"),
    };
    assert!(matches!(err, InferError::EmptyModelBytes));
    assert!(
        err.to_string().to_ascii_lowercase().contains("empty"),
        "empty-byte error must be explicit: {err}"
    );
}

#[test]
fn missing_path_rejected() {
    let missing = PathBuf::from("/tmp/holter-assist-missing-model-5-1.onnx");
    let source = ModelSource::Path(missing.clone());
    let err = match Phase2Model::load_from_source(&source, ExecutionProviderKind::Cpu) {
        Err(e) => e,
        Ok(_) => panic!("missing path must error"),
    };
    assert!(
        matches!(err, InferError::ModelNotFound(_)),
        "missing path must be ModelNotFound: {err}"
    );
    let msg = err.to_string();
    assert!(
        msg.contains(missing.to_string_lossy().as_ref()) || msg.contains("not found"),
        "error should identify the missing path: {msg}"
    );
}

/// Optional: feature-on builds with injected fixture/real model bytes build a session.
#[cfg(feature = "embedded-model")]
#[test]
fn embedded_session_builds_without_external_path() {
    use crate::phase2::WINDOW_SAMPLES;

    let mut model = Phase2Model::load_from_source(
        &ModelSource::Embedded,
        ExecutionProviderKind::Cpu,
    )
    .expect("embedded load_from_source (fixture or injected model)");
    assert_eq!(model.provider(), ExecutionProviderKind::Cpu);
    assert_eq!(model.model_path().to_string_lossy(), "<embedded>");
    let samples = {
        let mut s = vec![0.0_f32; WINDOW_SAMPLES];
        for (i, v) in s.iter_mut().enumerate() {
            *v = ((i % 97) as f32) * 0.01 - 0.5;
        }
        s
    };
    let out = model.infer_window(&samples).expect("embedded infer");
    assert_eq!(out.beat.len(), WINDOW_SAMPLES);
    assert_eq!(out.event.len(), WINDOW_SAMPLES);
}
