//! Accuracy / speed comparison between the baseline inference settings
//! (CPU, FP32, batch size 1, no CUDA tuning) and a candidate setting.
//!
//! Loads the candidate model first (aborting with the reason if its
//! execution provider is unavailable), then the baseline model, analyzes
//! each ECL with both, and collects accuracy differences and per-stage
//! timings into a comparison report. Must not depend on `http`.

pub mod metrics;
pub mod report;
