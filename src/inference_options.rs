//! Inference settings shared by the CLI and the `[http]` ini: batch size,
//! CUDA tuning switches, their key names, and the common value parsers.
//!
//! Leaf module: it must not depend on other crate modules. The full
//! `InferenceOptions` (with the execution provider) is assembled in `phase2`.

use std::num::NonZeroUsize;
use std::str::FromStr;
use thiserror::Error;

/// Windows per inference call when no batch size is configured.
pub const DEFAULT_BATCH_SIZE: usize = 16;
/// Largest accepted batch size.
pub const MAX_BATCH_SIZE: usize = 256;

/// Setting keys; CLI flags use the same names with `_` replaced by `-`.
pub const KEY_BATCH_SIZE: &str = "batch_size";
pub const KEY_CUDA_TF32: &str = "cuda_tf32";
pub const KEY_CUDA_CONV1D_PAD: &str = "cuda_conv1d_pad_to_nc1d";
pub const KEY_CUDA_GRAPH: &str = "cuda_graph";

/// Validation failures for inference settings.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum InferenceOptionsError {
    #[error("invalid '{key}': expected integer 1..={max}, got '{raw}'")]
    InvalidBatchSize {
        key: &'static str,
        raw: String,
        max: usize,
    },
    #[error("invalid '{key}': expected true|false|on|off|1|0, got '{raw}'")]
    InvalidSwitch { key: &'static str, raw: String },
}

/// Number of windows per inference call, always within `1..=MAX_BATCH_SIZE`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BatchSize(NonZeroUsize);

impl BatchSize {
    pub fn new(n: usize) -> Result<Self, InferenceOptionsError> {
        match NonZeroUsize::new(n) {
            Some(v) if n <= MAX_BATCH_SIZE => Ok(Self(v)),
            _ => Err(invalid_batch_size(n.to_string())),
        }
    }

    pub fn get(self) -> usize {
        self.0.get()
    }
}

impl Default for BatchSize {
    fn default() -> Self {
        Self::new(DEFAULT_BATCH_SIZE).expect("DEFAULT_BATCH_SIZE is within 1..=MAX_BATCH_SIZE")
    }
}

impl FromStr for BatchSize {
    type Err = InferenceOptionsError;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        s.trim()
            .parse::<usize>()
            .ok()
            .and_then(|n| Self::new(n).ok())
            .ok_or_else(|| invalid_batch_size(s.to_string()))
    }
}

fn invalid_batch_size(raw: String) -> InferenceOptionsError {
    InferenceOptionsError::InvalidBatchSize {
        key: KEY_BATCH_SIZE,
        raw,
        max: MAX_BATCH_SIZE,
    }
}

/// CUDA execution tuning. `None` means the option is not passed to ONNX
/// Runtime, keeping the pre-feature CUDA behavior.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct CudaTuning {
    pub tf32: Option<bool>,
    pub conv1d_pad_to_nc1d: Option<bool>,
    pub cuda_graph: Option<bool>,
}

impl CudaTuning {
    fn items(&self) -> [(&'static str, &'static str, Option<bool>); 3] {
        [
            (KEY_CUDA_TF32, "tf32", self.tf32),
            (
                KEY_CUDA_CONV1D_PAD,
                "conv1d_pad_to_nc1d",
                self.conv1d_pad_to_nc1d,
            ),
            (KEY_CUDA_GRAPH, "cuda_graph", self.cuda_graph),
        ]
    }

    /// True when no tuning item is specified.
    pub fn is_unset(&self) -> bool {
        self.items().iter().all(|(_, _, v)| v.is_none())
    }

    /// Setting keys (`KEY_CUDA_*`) of the specified items, in declaration order.
    pub fn specified_keys(&self) -> Vec<&'static str> {
        self.items()
            .iter()
            .filter(|(_, _, v)| v.is_some())
            .map(|(key, _, _)| *key)
            .collect()
    }

    /// e.g. `tf32=default,conv1d_pad_to_nc1d=on,cuda_graph=off`.
    pub fn describe(&self) -> String {
        self.items()
            .iter()
            .map(|(_, label, v)| {
                let state = match v {
                    None => "default",
                    Some(true) => "on",
                    Some(false) => "off",
                };
                format!("{label}={state}")
            })
            .collect::<Vec<_>>()
            .join(",")
    }

    pub fn wants_cuda_graph(&self) -> bool {
        self.cuda_graph == Some(true)
    }
}

/// Parse an on/off setting: `true|false|on|off|1|0`, case-insensitive,
/// surrounding whitespace ignored.
pub fn parse_switch(key: &'static str, raw: &str) -> Result<bool, InferenceOptionsError> {
    match raw.trim().to_ascii_lowercase().as_str() {
        "true" | "on" | "1" => Ok(true),
        "false" | "off" | "0" => Ok(false),
        _ => Err(InferenceOptionsError::InvalidSwitch {
            key,
            raw: raw.to_string(),
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn batch_size_rejects_zero_and_above_max() {
        for n in [0, MAX_BATCH_SIZE + 1, 257, usize::MAX] {
            let err = BatchSize::new(n).unwrap_err();
            assert_eq!(
                err,
                InferenceOptionsError::InvalidBatchSize {
                    key: KEY_BATCH_SIZE,
                    raw: n.to_string(),
                    max: MAX_BATCH_SIZE,
                }
            );
        }
    }

    #[test]
    fn batch_size_accepts_bounds_and_default() {
        for n in [1, 16, 256] {
            assert_eq!(BatchSize::new(n).unwrap().get(), n);
        }
        assert_eq!(BatchSize::default().get(), DEFAULT_BATCH_SIZE);
        assert_eq!(DEFAULT_BATCH_SIZE, 16);
        assert_eq!(MAX_BATCH_SIZE, 256);
    }

    #[test]
    fn batch_size_from_str_trims_and_validates() {
        assert_eq!("1".parse::<BatchSize>().unwrap().get(), 1);
        assert_eq!(" 32 ".parse::<BatchSize>().unwrap().get(), 32);
        assert_eq!("256".parse::<BatchSize>().unwrap().get(), 256);

        for raw in [
            "0",
            "257",
            "",
            "  ",
            "-1",
            "abc",
            "1.5",
            "99999999999999999999999",
        ] {
            let err = raw.parse::<BatchSize>().unwrap_err();
            assert_eq!(
                err,
                InferenceOptionsError::InvalidBatchSize {
                    key: KEY_BATCH_SIZE,
                    raw: raw.to_string(),
                    max: MAX_BATCH_SIZE,
                },
                "raw={raw:?}"
            );
        }
    }

    #[test]
    fn batch_size_error_message_names_key_range_and_value() {
        let msg = "abc".parse::<BatchSize>().unwrap_err().to_string();
        assert!(msg.contains("batch_size"), "{msg}");
        assert!(msg.contains("1..=256"), "{msg}");
        assert!(msg.contains("abc"), "{msg}");
    }

    #[test]
    fn parse_switch_accepts_documented_values_case_insensitively() {
        for raw in ["true", "TRUE", "True", "on", "ON", "On", "1", " true "] {
            assert_eq!(parse_switch(KEY_CUDA_TF32, raw), Ok(true), "raw={raw:?}");
        }
        for raw in [
            "false", "FALSE", "False", "off", "OFF", "Off", "0", "\toff\n",
        ] {
            assert_eq!(parse_switch(KEY_CUDA_TF32, raw), Ok(false), "raw={raw:?}");
        }
    }

    #[test]
    fn parse_switch_rejects_other_values_with_key_in_error() {
        for raw in ["", " ", "yes", "no", "2", "-1", "enable", "truee", "o n"] {
            let err = parse_switch(KEY_CUDA_GRAPH, raw).unwrap_err();
            assert_eq!(
                err,
                InferenceOptionsError::InvalidSwitch {
                    key: KEY_CUDA_GRAPH,
                    raw: raw.to_string(),
                },
                "raw={raw:?}"
            );
        }
        let msg = parse_switch(KEY_CUDA_CONV1D_PAD, "yes")
            .unwrap_err()
            .to_string();
        assert!(msg.contains("cuda_conv1d_pad_to_nc1d"), "{msg}");
        assert!(msg.contains("true|false|on|off|1|0"), "{msg}");
        assert!(msg.contains("yes"), "{msg}");
    }

    #[test]
    fn key_constants_match_documented_names() {
        assert_eq!(KEY_BATCH_SIZE, "batch_size");
        assert_eq!(KEY_CUDA_TF32, "cuda_tf32");
        assert_eq!(KEY_CUDA_CONV1D_PAD, "cuda_conv1d_pad_to_nc1d");
        assert_eq!(KEY_CUDA_GRAPH, "cuda_graph");
    }

    #[test]
    fn default_cuda_tuning_is_unset() {
        let t = CudaTuning::default();
        assert!(t.is_unset());
        assert!(t.specified_keys().is_empty());
        assert!(!t.wants_cuda_graph());
        assert_eq!(
            t.describe(),
            "tf32=default,conv1d_pad_to_nc1d=default,cuda_graph=default"
        );
    }

    #[test]
    fn cuda_tuning_reports_specified_items() {
        let t = CudaTuning {
            tf32: None,
            conv1d_pad_to_nc1d: Some(true),
            cuda_graph: Some(false),
        };
        assert!(!t.is_unset());
        assert_eq!(
            t.specified_keys(),
            vec![KEY_CUDA_CONV1D_PAD, KEY_CUDA_GRAPH]
        );
        assert_eq!(
            t.describe(),
            "tf32=default,conv1d_pad_to_nc1d=on,cuda_graph=off"
        );
        assert!(!t.wants_cuda_graph());

        let all = CudaTuning {
            tf32: Some(false),
            conv1d_pad_to_nc1d: Some(false),
            cuda_graph: Some(true),
        };
        assert!(!all.is_unset());
        assert_eq!(
            all.specified_keys(),
            vec![KEY_CUDA_TF32, KEY_CUDA_CONV1D_PAD, KEY_CUDA_GRAPH]
        );
        assert!(all.wants_cuda_graph());
        assert_eq!(
            all.describe(),
            "tf32=off,conv1d_pad_to_nc1d=off,cuda_graph=on"
        );
    }
}
