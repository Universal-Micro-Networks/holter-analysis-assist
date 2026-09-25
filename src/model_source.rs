//! Model load source contract (`ModelSource`).
//!
//! Distinguishes development path loading from distribution embedded loading.

use std::path::PathBuf;
use thiserror::Error;

/// Where the Phase-2 inference model is supplied from.
///
/// - [`Path`](ModelSource::Path): load from a filesystem path (development default).
/// - [`Embedded`](ModelSource::Embedded): use build-time embedded bytes (distribution).
///
/// Selecting [`Embedded`](ModelSource::Embedded) without the `embedded-model` Cargo
/// feature fails via [`ensure_supported`](ModelSource::ensure_supported).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelSource {
    /// Load the model from the given filesystem path.
    Path(PathBuf),
    /// Load the model from bytes embedded at build time (`embedded-model` feature).
    Embedded,
}

/// Errors from validating or resolving a [`ModelSource`].
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ModelSourceError {
    /// `ModelSource::Embedded` was selected but this build lacks `embedded-model`.
    #[error(
        "embedded model source requires the `embedded-model` Cargo feature \
         (rebuild with --features embedded-model)"
    )]
    EmbeddedUnavailable,
}

impl ModelSource {
    /// Returns `Ok(())` if this source can be used with the current build configuration.
    ///
    /// [`Path`](ModelSource::Path) is always supported at the type level (file existence
    /// is checked later by the loader). [`Embedded`](ModelSource::Embedded) requires
    /// the `embedded-model` feature.
    pub fn ensure_supported(&self) -> Result<(), ModelSourceError> {
        match self {
            Self::Path(_) => Ok(()),
            Self::Embedded => {
                if cfg!(feature = "embedded-model") {
                    Ok(())
                } else {
                    Err(ModelSourceError::EmbeddedUnavailable)
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn path_source_is_constructible_and_supported() {
        let path = PathBuf::from("resources/models/model.onnx");
        let src = ModelSource::Path(path.clone());
        assert!(matches!(&src, ModelSource::Path(p) if p == &path));
        assert!(src.ensure_supported().is_ok());
    }

    #[test]
    fn embedded_and_path_are_distinct_variants() {
        let path = ModelSource::Path(PathBuf::from("a.onnx"));
        let embedded = ModelSource::Embedded;
        assert_ne!(path, embedded);
        assert!(matches!(embedded, ModelSource::Embedded));
    }

    #[cfg(not(feature = "embedded-model"))]
    #[test]
    fn embedded_source_rejected_without_feature() {
        let err = ModelSource::Embedded.ensure_supported().unwrap_err();
        assert!(matches!(err, ModelSourceError::EmbeddedUnavailable));
        let msg = err.to_string();
        assert!(
            msg.contains("embedded-model"),
            "error should name the required feature: {msg}"
        );
    }

    #[cfg(feature = "embedded-model")]
    #[test]
    fn embedded_source_supported_with_feature() {
        assert!(ModelSource::Embedded.ensure_supported().is_ok());
    }
}
