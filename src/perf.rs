//! Per-job analysis performance measurements: stage timings (model load,
//! preprocess, inference, postprocess, output, total), processed window
//! count, and the effective inference settings, formatted as a single
//! diagnostic `perf:` log line.
//!
//! The log line always goes to stderr and is never mixed into the analysis
//! result bodies (CSV / JSON).
