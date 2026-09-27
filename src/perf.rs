//! Per-job analysis performance measurements: stage timings (model load,
//! preprocess, inference, postprocess, output, total), processed window
//! count, and the effective inference settings, formatted as a single
//! diagnostic `perf:` log line.
//!
//! The log line always goes to stderr and is never mixed into the analysis
//! result bodies (CSV / JSON).

use crate::phase2::{EffectiveInference, ModelBatchShape};
use std::time::Duration;

/// Placeholder value for items not measured or not known for this job.
const ABSENT: &str = "-";
/// `cuda_tuning` value when the tuning was not passed to the CUDA EP.
const NOT_APPLIED: &str = "not_applied";

/// `provider=.. batch_size=.. model_batch=.. cuda_tuning=..` tokens of the
/// effective inference settings, shared by the `perf:` line and the model
/// ready log.
///
/// `model_batch` is `dynamic` or `fixed:N`. `cuda_tuning` is
/// [`CudaTuning::describe`](crate::inference_options::CudaTuning::describe)
/// only when the tuning was applied (`cuda_applied`), otherwise
/// `not_applied`. With no effective settings every value is `-`.
pub fn effective_fields(effective: Option<&EffectiveInference>) -> String {
    let (provider, batch_size, model_batch, cuda_tuning) = match effective {
        Some(eff) => (
            eff.provider.as_str().to_string(),
            eff.batch_size.to_string(),
            model_batch_label(eff.model_batch),
            if eff.cuda_applied {
                eff.cuda.describe()
            } else {
                NOT_APPLIED.to_string()
            },
        ),
        None => (
            ABSENT.to_string(),
            ABSENT.to_string(),
            ABSENT.to_string(),
            ABSENT.to_string(),
        ),
    };
    format!(
        "provider={provider} batch_size={batch_size} model_batch={model_batch} \
         cuda_tuning={cuda_tuning}"
    )
}

/// Wall-clock time spent in each analysis stage of one job.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct StageTimings {
    /// Only when the job itself loaded the model.
    pub model_load: Option<Duration>,
    /// ECL read + continuous AI signal generation.
    pub preprocess: Duration,
    /// Window materialization + `infer_batch` + candidate extraction.
    pub inference: Duration,
    /// Clustering through Unknown QC and RUN finalization.
    pub postprocess: Duration,
    /// Row generation + CSV write + consistency checks.
    pub output: Duration,
    pub total: Duration,
}

/// Measurements of one analysis job.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct AnalyzePerf {
    pub timings: StageTimings,
    pub windows: usize,
    pub effective: Option<EffectiveInference>,
}

impl AnalyzePerf {
    /// Windows per second of total job time; 0 when no time was recorded.
    pub fn windows_per_sec_total(&self) -> f64 {
        windows_per_sec(self.windows, self.timings.total)
    }

    /// Windows per second of inference-stage time; 0 when no time was recorded.
    pub fn windows_per_sec_inference(&self) -> f64 {
        windows_per_sec(self.windows, self.timings.inference)
    }

    /// Single-line diagnostic for stderr, e.g.
    /// `perf: provider=cuda batch_size=16 model_batch=dynamic cuda_tuning=tf32=default,conv1d_pad_to_nc1d=on,cuda_graph=off windows=2064 model_load_ms=812.3 preprocess_ms=... inference_ms=... postprocess_ms=... output_ms=... total_ms=... windows_per_s=... windows_per_s_inference=...`.
    ///
    /// The settings tokens follow [`effective_fields`]. A `model_load` that
    /// was not measured is written as `-`.
    pub fn log_line(&self) -> String {
        let t = &self.timings;
        let model_load = t.model_load.map_or_else(|| ABSENT.to_string(), fmt_ms);
        format!(
            "perf: {settings} windows={windows} model_load_ms={model_load} \
             preprocess_ms={preprocess} inference_ms={inference} postprocess_ms={postprocess} \
             output_ms={output} total_ms={total} windows_per_s={wps_total:.1} \
             windows_per_s_inference={wps_inference:.1}",
            settings = effective_fields(self.effective.as_ref()),
            windows = self.windows,
            preprocess = fmt_ms(t.preprocess),
            inference = fmt_ms(t.inference),
            postprocess = fmt_ms(t.postprocess),
            output = fmt_ms(t.output),
            total = fmt_ms(t.total),
            wps_total = self.windows_per_sec_total(),
            wps_inference = self.windows_per_sec_inference(),
        )
    }

    /// Write [`Self::log_line`] to stderr (never stdout, CSV or JSON).
    pub fn emit_stderr(&self) {
        eprintln!("{}", self.log_line());
    }
}

fn windows_per_sec(windows: usize, elapsed: Duration) -> f64 {
    let secs = elapsed.as_secs_f64();
    if secs > 0.0 {
        windows as f64 / secs
    } else {
        0.0
    }
}

fn fmt_ms(d: Duration) -> String {
    format!("{:.1}", d.as_secs_f64() * 1000.0)
}

fn model_batch_label(shape: ModelBatchShape) -> String {
    match shape {
        ModelBatchShape::Dynamic => "dynamic".to_string(),
        ModelBatchShape::Fixed(n) => format!("fixed:{n}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inference_options::CudaTuning;
    use crate::phase2::ExecutionProviderKind;

    fn cuda_effective() -> EffectiveInference {
        EffectiveInference {
            provider: ExecutionProviderKind::Cuda,
            batch_size: 16,
            requested_batch_size: 16,
            model_batch: ModelBatchShape::Dynamic,
            pad_tail: false,
            cuda: CudaTuning {
                tf32: None,
                conv1d_pad_to_nc1d: Some(true),
                cuda_graph: Some(false),
            },
            cuda_applied: true,
            cuda_graph_active: false,
            notes: Vec::new(),
        }
    }

    fn sample_perf() -> AnalyzePerf {
        AnalyzePerf {
            timings: StageTimings {
                model_load: Some(Duration::from_micros(812_300)),
                preprocess: Duration::from_millis(1_500),
                inference: Duration::from_millis(4_000),
                postprocess: Duration::from_micros(250_240),
                output: Duration::from_millis(125),
                total: Duration::from_millis(8_000),
            },
            windows: 2064,
            effective: Some(cuda_effective()),
        }
    }

    fn fields(line: &str) -> Vec<(&str, &str)> {
        line.strip_prefix("perf: ")
            .expect("line starts with 'perf: '")
            .split(' ')
            .map(|tok| tok.split_once('=').expect("key=value token"))
            .collect()
    }

    fn field<'a>(line: &'a str, key: &str) -> &'a str {
        fields(line)
            .into_iter()
            .find(|(k, _)| *k == key)
            .map(|(_, v)| v)
            .unwrap_or_else(|| panic!("missing '{key}' in {line}"))
    }

    #[test]
    fn windows_per_sec_uses_total_and_inference_durations() {
        let perf = sample_perf();
        assert!((perf.windows_per_sec_total() - 258.0).abs() < 1e-9);
        assert!((perf.windows_per_sec_inference() - 516.0).abs() < 1e-9);
    }

    #[test]
    fn windows_per_sec_is_zero_for_zero_duration() {
        let perf = AnalyzePerf {
            windows: 10,
            ..AnalyzePerf::default()
        };
        assert_eq!(perf.windows_per_sec_total(), 0.0);
        assert_eq!(perf.windows_per_sec_inference(), 0.0);
        assert!(perf.log_line().contains("windows_per_s=0.0"));
    }

    #[test]
    fn log_line_contains_all_required_items_in_order() {
        let line = sample_perf().log_line();
        assert_eq!(
            line,
            "perf: provider=cuda batch_size=16 model_batch=dynamic \
             cuda_tuning=tf32=default,conv1d_pad_to_nc1d=on,cuda_graph=off \
             windows=2064 model_load_ms=812.3 preprocess_ms=1500.0 \
             inference_ms=4000.0 postprocess_ms=250.2 output_ms=125.0 \
             total_ms=8000.0 windows_per_s=258.0 windows_per_s_inference=516.0"
        );
    }

    #[test]
    fn log_line_fields_are_parseable_key_value_tokens() {
        let line = sample_perf().log_line();
        let keys: Vec<&str> = fields(&line).into_iter().map(|(k, _)| k).collect();
        assert_eq!(
            keys,
            [
                "provider",
                "batch_size",
                "model_batch",
                "cuda_tuning",
                "windows",
                "model_load_ms",
                "preprocess_ms",
                "inference_ms",
                "postprocess_ms",
                "output_ms",
                "total_ms",
                "windows_per_s",
                "windows_per_s_inference",
            ]
        );
    }

    #[test]
    fn log_line_uses_effective_batch_size_and_model_shape() {
        let mut perf = sample_perf();
        let eff = perf.effective.as_mut().unwrap();
        eff.provider = ExecutionProviderKind::Cpu;
        eff.batch_size = 1;
        eff.requested_batch_size = 16;
        eff.model_batch = ModelBatchShape::Fixed(1);
        eff.cuda = CudaTuning::default();
        eff.cuda_applied = false;
        let line = perf.log_line();
        assert_eq!(field(&line, "provider"), "cpu");
        assert_eq!(field(&line, "batch_size"), "1");
        assert_eq!(field(&line, "model_batch"), "fixed:1");
        assert_eq!(field(&line, "cuda_tuning"), "not_applied");
    }

    #[test]
    fn log_line_reports_describe_when_cuda_tuning_applied() {
        let perf = sample_perf();
        let eff = perf.effective.as_ref().unwrap();
        assert!(eff.cuda_applied);
        assert_eq!(field(&perf.log_line(), "cuda_tuning"), eff.cuda.describe());
    }

    #[test]
    fn effective_fields_is_the_settings_part_of_log_line() {
        let perf = sample_perf();
        let settings = effective_fields(perf.effective.as_ref());
        assert_eq!(
            settings,
            "provider=cuda batch_size=16 model_batch=dynamic \
             cuda_tuning=tf32=default,conv1d_pad_to_nc1d=on,cuda_graph=off"
        );
        assert!(perf.log_line().starts_with(&format!("perf: {settings} ")));
        assert_eq!(
            effective_fields(None),
            "provider=- batch_size=- model_batch=- cuda_tuning=-"
        );
    }

    #[test]
    fn log_line_does_not_report_requested_tuning_when_not_applied() {
        let mut perf = sample_perf();
        let eff = perf.effective.as_mut().unwrap();
        eff.provider = ExecutionProviderKind::Cpu;
        eff.cuda = CudaTuning {
            tf32: Some(true),
            conv1d_pad_to_nc1d: Some(true),
            cuda_graph: Some(true),
        };
        eff.cuda_applied = false;
        let line = perf.log_line();
        assert_eq!(field(&line, "provider"), "cpu");
        assert_eq!(field(&line, "cuda_tuning"), "not_applied");
        for applied in ["tf32=on", "conv1d_pad_to_nc1d=on", "cuda_graph=on"] {
            assert!(!line.contains(applied), "{applied} in {line}");
        }
    }

    #[test]
    fn log_line_marks_absent_model_load_and_effective_settings() {
        let mut perf = sample_perf();
        perf.timings.model_load = None;
        perf.effective = None;
        let line = perf.log_line();
        for key in ["provider", "batch_size", "model_batch", "cuda_tuning"] {
            assert_eq!(field(&line, key), "-", "{key}");
        }
        assert_eq!(field(&line, "model_load_ms"), "-");
        assert_eq!(field(&line, "windows"), "2064");
        assert_eq!(field(&line, "inference_ms"), "4000.0");
    }

    #[test]
    fn log_line_is_single_line_diagnostic_not_json_or_csv() {
        let line = sample_perf().log_line();
        assert!(line.starts_with("perf: "), "{line}");
        assert!(!line.contains('\n') && !line.contains('\r'), "{line}");
        assert!(!line.contains('{') && !line.contains('"'), "{line}");
        assert!(!line.contains(';'), "{line}");
    }
}
