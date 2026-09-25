//! Runtime access to build-time embedded model bytes (`EmbeddedModelBytes`).
//!
//! Compiled only with `--features embedded-model`. Bytes come from
//! `OUT_DIR/embedded_model.bin` produced by `build.rs`.

/// Returns the model bytes embedded at build time.
///
/// Non-empty when `HOLTER_EMBEDDED_MODEL_PATH` pointed at a valid file during build.
#[cfg(feature = "embedded-model")]
pub fn embedded_model_bytes() -> &'static [u8] {
    include_bytes!(concat!(env!("OUT_DIR"), "/embedded_model.bin"))
}

#[cfg(all(test, feature = "embedded-model"))]
mod tests {
    use super::*;

    #[test]
    fn embedded_model_bytes_are_non_empty() {
        let bytes = embedded_model_bytes();
        assert!(
            !bytes.is_empty(),
            "embedded model bytes must be non-empty when feature embedded-model is enabled"
        );
    }
}
