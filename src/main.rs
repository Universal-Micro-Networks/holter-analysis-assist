use clap::{Args, Parser, Subcommand, ValueEnum};
use holter_analysis_assist::accel_compare::report::{
    to_markdown, CompareReport, Thresholds as CompareThresholds,
};
use holter_analysis_assist::accel_compare::{
    run_compare, CompareConfig, DEFAULT_PROB_STRIDE, DEFAULT_REPORT_DIR, DEFAULT_TOLERANCE_SAMPLES,
};
use holter_analysis_assist::analyze::analyze_ecl_with_source;
use holter_analysis_assist::inference_options::{
    parse_switch, BatchSize, CudaTuning, InferenceOptionsError, KEY_CUDA_CONV1D_PAD,
    KEY_CUDA_GRAPH, KEY_CUDA_TF32,
};
use holter_analysis_assist::license::{
    LicenseConfig, LicenseError, LicenseGate, ReqwestLicenseClient,
};
use holter_analysis_assist::phase2::{
    self, ExecutionProviderKind, InferenceOptions, Phase2Model, WINDOW_SAMPLES,
};
use holter_analysis_assist::{classify_ecg, ClassificationResult, ModelSource};
use serde::Serialize;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Development default when `--model` is omitted and `embedded-model` is off.
const DEFAULT_DEV_MODEL: &str = "resources/models/phase2_rev1.onnx";

/// Resolve CLI `--model` into a [`ModelSource`] (CliModelSelect).
///
/// - `Some(path)` → always [`ModelSource::Path`]
/// - `None` + `embedded-model` feature → [`ModelSource::Embedded`]
/// - `None` without feature → Path to [`DEFAULT_DEV_MODEL`]
fn resolve_model_source(model: Option<PathBuf>) -> ModelSource {
    match model {
        Some(path) => ModelSource::Path(path),
        None if cfg!(feature = "embedded-model") => ModelSource::Embedded,
        None => ModelSource::Path(PathBuf::from(DEFAULT_DEV_MODEL)),
    }
}

/// Default relative path when neither `--license-config` nor `HOLTER_LICENSE_INI` is set.
const DEFAULT_LICENSE_INI: &str = "config/license.ini";

#[derive(Parser, Debug)]
#[command(
    name = "holter-analysis-assist",
    version,
    about = "Holter ECG arrhythmia assist — CLI stub + Phase-2 ONNX reference"
)]
struct Cli {
    /// Path to license.ini (`[license]`). Overrides `HOLTER_LICENSE_INI`; default `config/license.ini`.
    #[arg(
        long = "license-config",
        global = true,
        env = "HOLTER_LICENSE_INI",
        default_value = DEFAULT_LICENSE_INI,
        value_name = "PATH"
    )]
    license_config: PathBuf,

    #[command(subcommand)]
    command: Commands,
}

/// Load ini → build HTTP client → install process-wide Gate → startup validity check.
///
/// Order matches CliStartupIntegration (design): load → construct → install → ensure_startup_licensed.
/// Failures are fail-closed (no offline bypass); caller must exit before any subcommand.
fn install_and_ensure_startup_licensed(ini_path: &Path) -> Result<(), LicenseError> {
    let config = LicenseConfig::load_from_path(ini_path)?;
    let client = ReqwestLicenseClient::new(config)?;
    LicenseGate::install(LicenseGate::new(client))?;
    LicenseGate::global().ensure_startup_licensed()
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Classify an ECG input file (stub inference for now).
    Classify {
        /// Path to an ECG input file.
        #[arg(value_name = "INPUT")]
        input: PathBuf,

        /// Output format.
        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,
    },

    /// Run Phase-2 ONNX inference on one 20s @ 500Hz window (10_000 float32 samples).
    InferWindow {
        /// ONNX model path. Omitted: embedded model (embedded-model build) or
        /// `resources/models/phase2_rev1.onnx` (development default).
        #[arg(long)]
        model: Option<PathBuf>,

        /// Raw float32 little-endian window file (40_000 bytes). If omitted, uses a synthetic sine.
        #[arg(long)]
        input: Option<PathBuf>,

        /// Apply external per-window z-score before inference (BeatSense preprocess).
        #[arg(long, default_value_t = true)]
        zscore: bool,

        #[arg(long, value_enum, default_value_t = OutputFormat::Text)]
        format: OutputFormat,

        /// ONNX Runtime EP: `auto` (CUDA→CPU), `cuda`, or `cpu`.
        #[arg(long, value_enum, default_value_t = ProviderArg::Auto)]
        provider: ProviderArg,
    },

    /// Analyze a full ECL file → beat_results.csv (preprocess + ONNX + postprocess).
    AnalyzeEcl {
        /// Input ECL path (`[serial]_[yyyyMMdd]_[HHmm]_[HHmm].ecl`).
        #[arg(value_name = "ECL")]
        ecl: PathBuf,

        /// Phase-2 ONNX model path. Omitted: embedded model (embedded-model build) or
        /// `resources/models/phase2_rev1.onnx` (development default).
        #[arg(long)]
        model: Option<PathBuf>,

        /// Output CSV path.
        #[arg(long, default_value = "output/beat_results.csv")]
        output: PathBuf,

        /// Optional cap on number of 20s windows (smoke / debug).
        #[arg(long)]
        max_windows: Option<usize>,

        #[command(flatten)]
        inference: InferenceArgs,
    },

    /// Compare a candidate inference setting against the baseline (CPU, FP32,
    /// batch size 1, no CUDA tuning) on the same ECLs.
    ///
    /// Writes `report.json`, `report.md` and per-ECL `baseline.csv` /
    /// `candidate.csv` into `--report-dir` and prints the Markdown summary to
    /// stdout. Each ECL is analyzed twice, so the license is metered twice per ECL.
    #[command(after_help = COMPARE_EXIT_CODES)]
    CompareAccel(CompareAccelArgs),

    /// Start the HTTP API + console UI (same as `holter-http-api`).
    ///
    /// Use this when Device Guard blocks `holter-http-api.exe`. Config must
    /// contain both `[http]` and `[license]` (e.g. `config/http.ini`).
    ServeHttp {
        /// Path to ini containing `[http]` and `[license]` sections.
        #[arg(
            long = "config",
            env = "HOLTER_HTTP_INI",
            default_value = "config/http.ini",
            value_name = "PATH"
        )]
        config: PathBuf,
    },
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

#[derive(Clone, Copy, Debug, ValueEnum, Default)]
enum ProviderArg {
    #[default]
    Auto,
    Cpu,
    Cuda,
}

impl From<ProviderArg> for ExecutionProviderKind {
    fn from(value: ProviderArg) -> Self {
        match value {
            ProviderArg::Auto => Self::Auto,
            ProviderArg::Cpu => Self::Cpu,
            ProviderArg::Cuda => Self::Cuda,
        }
    }
}

/// Inference settings shared by analysis subcommands. Flag names are the
/// `[http]` ini keys with `_` replaced by `-`, and values use the same parsers.
#[derive(Args, Debug, Clone, Copy)]
struct InferenceArgs {
    /// ONNX Runtime EP: `auto` (CUDA→CPU), `cuda`, or `cpu`.
    #[arg(long, value_enum, default_value_t = ProviderArg::Auto)]
    provider: ProviderArg,

    /// Windows per inference call: integer 1..=256 (`[http] batch_size`). Default: 16.
    #[arg(
        long = "batch-size",
        value_name = "N",
        value_parser = parse_batch_size,
        allow_negative_numbers = true
    )]
    batch_size: Option<BatchSize>,

    /// CUDA TF32 math: true|false|on|off|1|0 (`[http] cuda_tf32`). Omitted: ONNX Runtime default.
    #[arg(long = "cuda-tf32", value_name = "SWITCH", value_parser = parse_cuda_tf32)]
    cuda_tf32: Option<bool>,

    /// CUDA Conv1D pad-to-NC1D: true|false|on|off|1|0 (`[http] cuda_conv1d_pad_to_nc1d`).
    /// Omitted: ONNX Runtime default.
    #[arg(
        long = "cuda-conv1d-pad-to-nc1d",
        value_name = "SWITCH",
        value_parser = parse_cuda_conv1d_pad
    )]
    cuda_conv1d_pad_to_nc1d: Option<bool>,

    /// CUDA graph capture: true|false|on|off|1|0 (`[http] cuda_graph`). Omitted: ONNX Runtime default.
    #[arg(long = "cuda-graph", value_name = "SWITCH", value_parser = parse_cuda_graph)]
    cuda_graph: Option<bool>,
}

impl InferenceArgs {
    fn to_options(self) -> InferenceOptions {
        InferenceOptions {
            provider: self.provider.into(),
            batch_size: self.batch_size.unwrap_or_default(),
            cuda: CudaTuning {
                tf32: self.cuda_tf32,
                conv1d_pad_to_nc1d: self.cuda_conv1d_pad_to_nc1d,
                cuda_graph: self.cuda_graph,
            },
        }
    }
}

fn parse_batch_size(raw: &str) -> Result<BatchSize, InferenceOptionsError> {
    raw.parse()
}

fn parse_cuda_tf32(raw: &str) -> Result<bool, InferenceOptionsError> {
    parse_switch(KEY_CUDA_TF32, raw)
}

fn parse_cuda_conv1d_pad(raw: &str) -> Result<bool, InferenceOptionsError> {
    parse_switch(KEY_CUDA_CONV1D_PAD, raw)
}

fn parse_cuda_graph(raw: &str) -> Result<bool, InferenceOptionsError> {
    parse_switch(KEY_CUDA_GRAPH, raw)
}

const COMPARE_EXIT_CODES: &str = "\
Exit codes:
  0 = comparison finished and every given threshold passed (or none was given)
  1 = error (license, model / candidate provider unavailable, analysis, I/O)
  2 = comparison finished but a given threshold failed
      (invalid command-line arguments also exit with 2, before any analysis)";

#[derive(Args, Debug, Clone)]
struct CompareAccelArgs {
    /// Input ECL paths (one or more).
    #[arg(value_name = "ECL", required = true, num_args = 1..)]
    ecls: Vec<PathBuf>,

    /// Phase-2 ONNX model path, used for both baseline and candidate. Omitted:
    /// embedded model (embedded-model build) or `resources/models/phase2_rev1.onnx`
    /// (development default).
    #[arg(long)]
    model: Option<PathBuf>,

    // Candidate inference settings: same flags and values as `analyze-ecl`.
    #[command(flatten)]
    inference: InferenceArgs,

    /// Beat matching tolerance in 500 Hz samples (integer >= 0; 40 = 80 ms).
    #[arg(
        long = "tolerance-samples",
        value_name = "N",
        default_value_t = DEFAULT_TOLERANCE_SAMPLES,
        allow_negative_numbers = true
    )]
    tolerance_samples: u32,

    /// Compare beat / event probabilities on every N-th window (integer >= 1);
    /// rhythm scores are compared on every window. Baseline outputs of every
    /// N-th window are kept in memory (~160 KB each: ~800 MB for 24 h at 1,
    /// ~5.7 GB for 7 days); use a larger N for multi-day ECLs.
    #[arg(
        long = "prob-stride",
        value_name = "N",
        default_value_t = DEFAULT_PROB_STRIDE,
        value_parser = parse_positive_count,
        allow_negative_numbers = true
    )]
    prob_stride: usize,

    /// Optional cap on number of 20s windows per ECL (integer >= 1).
    #[arg(
        long = "max-windows",
        value_name = "N",
        value_parser = parse_positive_count,
        allow_negative_numbers = true
    )]
    max_windows: Option<usize>,

    /// Output directory for the report and per-ECL CSVs (existing files are overwritten).
    #[arg(long = "report-dir", value_name = "DIR", default_value = DEFAULT_REPORT_DIR)]
    report_dir: PathBuf,

    #[command(flatten)]
    thresholds: ThresholdArgs,
}

/// Optional pass/fail thresholds on the aggregate metrics. No verdict is
/// reported unless at least one is given.
#[derive(Args, Debug, Clone, Copy)]
struct ThresholdArgs {
    /// Minimum rhythm (SR vs AF/AFL) window agreement rate, 0..=1.
    #[arg(
        long = "min-rhythm-window-agreement",
        value_name = "RATE",
        value_parser = parse_rate,
        allow_negative_numbers = true
    )]
    min_rhythm_window_agreement: Option<f64>,

    /// Minimum matched beat rate (lower of vs-baseline and vs-candidate), 0..=1.
    #[arg(
        long = "min-beat-match-rate",
        value_name = "RATE",
        value_parser = parse_rate,
        allow_negative_numbers = true
    )]
    min_beat_match_rate: Option<f64>,

    /// Minimum label agreement rate of matched beats, 0..=1.
    #[arg(
        long = "min-beat-class-agreement",
        value_name = "RATE",
        value_parser = parse_rate,
        allow_negative_numbers = true
    )]
    min_beat_class_agreement: Option<f64>,

    /// Maximum absolute probability difference (beat / event / rhythm), >= 0.
    #[arg(
        long = "max-prob-abs-diff",
        value_name = "X",
        value_parser = parse_non_negative,
        allow_negative_numbers = true
    )]
    max_prob_abs_diff: Option<f64>,

    /// Maximum matched beat offset in 500 Hz samples, >= 0.
    #[arg(
        long = "max-offset-samples",
        value_name = "X",
        value_parser = parse_non_negative,
        allow_negative_numbers = true
    )]
    max_offset_samples: Option<f64>,
}

impl CompareAccelArgs {
    fn into_config(self) -> CompareConfig {
        let t = self.thresholds;
        CompareConfig {
            ecl_paths: self.ecls,
            model: resolve_model_source(self.model),
            candidate: self.inference.to_options(),
            tolerance_samples: self.tolerance_samples,
            prob_stride: self.prob_stride,
            max_windows: self.max_windows,
            thresholds: CompareThresholds {
                min_rhythm_window_agreement: t.min_rhythm_window_agreement,
                min_beat_match_rate: t.min_beat_match_rate,
                min_beat_class_agreement: t.min_beat_class_agreement,
                max_prob_abs_diff: t.max_prob_abs_diff,
                max_offset_samples: t.max_offset_samples,
            },
            report_dir: self.report_dir,
        }
    }
}

fn parse_positive_count(raw: &str) -> Result<usize, String> {
    match raw.trim().parse::<usize>() {
        Ok(n) if n >= 1 => Ok(n),
        _ => Err("must be an integer >= 1".to_string()),
    }
}

fn parse_finite(raw: &str) -> Option<f64> {
    raw.trim().parse::<f64>().ok().filter(|x| x.is_finite())
}

fn parse_rate(raw: &str) -> Result<f64, String> {
    parse_finite(raw)
        .filter(|x| (0.0..=1.0).contains(x))
        .ok_or_else(|| "must be a finite number in 0..=1".to_string())
}

fn parse_non_negative(raw: &str) -> Result<f64, String> {
    parse_finite(raw)
        .filter(|x| *x >= 0.0)
        .ok_or_else(|| "must be a finite number >= 0".to_string())
}

/// Process exit status of a finished comparison ([`CompareReport::exit_code`]).
fn compare_exit_status(report: &CompareReport) -> u8 {
    u8::try_from(report.exit_code()).unwrap_or(1)
}

fn run_compare_accel(args: CompareAccelArgs) -> ExitCode {
    match run_compare(&args.into_config()) {
        Ok(report) => {
            print!("{}", to_markdown(&report));
            ExitCode::from(compare_exit_status(&report))
        }
        Err(err) => {
            eprintln!("error: {err}");
            ExitCode::FAILURE
        }
    }
}

#[derive(Serialize)]
struct InferWindowReport {
    model: String,
    provider: String,
    rhythm_score: f32,
    rhythm_class: String,
    summary_beat_class: String,
    beat_mean: f32,
    event_mean_pac: f32,
    event_mean_pvc: f32,
    event_mean_n: f32,
    thresholds: Thresholds,
}

#[derive(Serialize)]
struct Thresholds {
    beat: f32,
    pac: f32,
    pvc: f32,
    af: f32,
}

fn main() -> ExitCode {
    let cli = Cli::parse();

    // HTTP serve owns its own license install from the merged http ini — skip the
    // CLI-default license.ini path so Gate is not installed twice.
    if let Commands::ServeHttp { config } = &cli.command {
        return match holter_analysis_assist::http::run_blocking(config) {
            Ok(()) => ExitCode::SUCCESS,
            Err(err) => {
                eprintln!("error: {err}");
                ExitCode::FAILURE
            }
        };
    }

    if let Err(err) = install_and_ensure_startup_licensed(&cli.license_config) {
        // StartupFailed / Config / install errors: identifiable; do not run subcommands.
        eprintln!("error: {err}");
        return ExitCode::FAILURE;
    }

    match cli.command {
        Commands::ServeHttp { .. } => unreachable!("handled above"),
        Commands::Classify { input, format } => match classify_ecg(&input) {
            Ok(result) => {
                print_classify(&result, format);
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("error: {err}");
                ExitCode::FAILURE
            }
        },
        Commands::InferWindow {
            model,
            input,
            zscore,
            format,
            provider,
        } => {
            let source = resolve_model_source(model);
            match run_infer_window(source, input, zscore, format, provider.into()) {
                Ok(()) => ExitCode::SUCCESS,
                Err(err) => {
                    eprintln!("error: {err}");
                    ExitCode::FAILURE
                }
            }
        }
        Commands::AnalyzeEcl {
            ecl,
            model,
            output,
            max_windows,
            inference,
        } => {
            let source = resolve_model_source(model);
            let options = inference.to_options();
            match analyze_ecl_with_source(&ecl, &source, &output, max_windows, &options) {
                Ok((_rows, summary)) => {
                    println!("saved: {}", output.display());
                    println!("beats: {}", summary.beats);
                    println!("windows: {}", summary.windows);
                    println!("Unknown=1: {}", summary.unknown_ones);
                    println!("short_run_flag=1: {}", summary.short_run_ones);
                    summary.perf.emit_stderr();
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("error: {err}");
                    ExitCode::FAILURE
                }
            }
        }
        Commands::CompareAccel(args) => run_compare_accel(args),
    }
}

fn run_infer_window(
    source: ModelSource,
    input: Option<PathBuf>,
    zscore: bool,
    format: OutputFormat,
    provider: ExecutionProviderKind,
) -> Result<(), Box<dyn std::error::Error>> {
    let mut samples = match input {
        Some(path) => load_f32_window(&path)?,
        None => synthetic_window(),
    };
    if zscore {
        phase2::zscore_window(&mut samples);
    }

    let mut model = Phase2Model::load_from_source(&source, provider)?;
    let resolved = model.provider();
    let out = model.infer_window(&samples)?;

    let beat_mean = out.beat.iter().sum::<f32>() / out.beat.len() as f32;
    let mut event_sum = [0.0_f32; 3];
    for row in &out.event {
        event_sum[0] += row[0];
        event_sum[1] += row[1];
        event_sum[2] += row[2];
    }
    let n = out.event.len() as f32;
    let report = InferWindowReport {
        model: model.model_path().display().to_string(),
        provider: resolved.as_str().to_string(),
        rhythm_score: out.rhythm,
        rhythm_class: out.rhythm_class().as_str().to_string(),
        summary_beat_class: out.summary_beat_class().as_str().to_string(),
        beat_mean,
        event_mean_pac: event_sum[0] / n,
        event_mean_pvc: event_sum[1] / n,
        event_mean_n: event_sum[2] / n,
        thresholds: Thresholds {
            beat: phase2::TH_BEAT,
            pac: phase2::TH_PAC,
            pvc: phase2::TH_PVC,
            af: phase2::TH_AF,
        },
    };

    match format {
        OutputFormat::Text => {
            println!("model:              {}", report.model);
            println!("provider:           {}", report.provider);
            println!("rhythm_score:       {:.4}", report.rhythm_score);
            println!("rhythm_class:       {}", report.rhythm_class);
            println!("summary_beat_class: {}", report.summary_beat_class);
            println!("beat_mean:          {:.4}", report.beat_mean);
            println!(
                "event_mean:         PAC={:.4} PVC={:.4} N={:.4}",
                report.event_mean_pac, report.event_mean_pvc, report.event_mean_n
            );
        }
        OutputFormat::Json => {
            println!("{}", serde_json::to_string_pretty(&report)?);
        }
    }
    Ok(())
}

fn load_f32_window(path: &PathBuf) -> Result<Vec<f32>, Box<dyn std::error::Error>> {
    let bytes = fs::read(path)?;
    if bytes.len() != WINDOW_SAMPLES * 4 {
        return Err(format!(
            "expected {} bytes ({} float32 LE samples), got {}",
            WINDOW_SAMPLES * 4,
            WINDOW_SAMPLES,
            bytes.len()
        )
        .into());
    }
    let mut samples = Vec::with_capacity(WINDOW_SAMPLES);
    for chunk in bytes.chunks_exact(4) {
        samples.push(f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]));
    }
    Ok(samples)
}

fn synthetic_window() -> Vec<f32> {
    // ~1.2 Hz sine @ 500 Hz — enough for a deterministic smoke path.
    (0..WINDOW_SAMPLES)
        .map(|i| {
            let t = i as f32 / phase2::MODEL_FS_HZ as f32;
            (2.0 * std::f32::consts::PI * 1.2 * t).sin()
        })
        .collect()
}

fn print_classify(result: &ClassificationResult, format: OutputFormat) {
    match format {
        OutputFormat::Text => {
            println!("input:      {}", result.input_path);
            println!("label:      {}", result.label);
            println!("confidence: {:.3}", result.confidence);
            println!("notes:      {}", result.notes);
        }
        OutputFormat::Json => {
            println!(
                "{}",
                serde_json::to_string_pretty(result).expect("serialize ClassificationResult")
            );
        }
    }
}

#[cfg(test)]
mod cli_inference_args_tests {
    use super::*;
    use clap::CommandFactory;
    use holter_analysis_assist::inference_options::{DEFAULT_BATCH_SIZE, KEY_BATCH_SIZE};

    fn parse_analyze(extra: &[&str]) -> Result<InferenceOptions, clap::Error> {
        let mut argv = vec!["holter-analysis-assist", "analyze-ecl", "x.ecl"];
        argv.extend_from_slice(extra);
        match Cli::try_parse_from(argv)?.command {
            Commands::AnalyzeEcl { inference, .. } => Ok(inference.to_options()),
            other => panic!("unexpected subcommand {other:?}"),
        }
    }

    #[test]
    fn flag_names_are_ini_keys_with_dashes() {
        let cmd = Cli::command();
        let analyze = cmd
            .find_subcommand("analyze-ecl")
            .expect("analyze-ecl subcommand");
        let longs: Vec<&str> = analyze
            .get_arguments()
            .filter_map(|a| a.get_long())
            .collect();
        for key in [
            KEY_BATCH_SIZE,
            KEY_CUDA_TF32,
            KEY_CUDA_CONV1D_PAD,
            KEY_CUDA_GRAPH,
        ] {
            let long = key.replace('_', "-");
            assert!(
                longs.contains(&long.as_str()),
                "missing --{long} in {longs:?}"
            );
        }
    }

    #[test]
    fn omitted_options_keep_pre_feature_defaults() {
        let opts = parse_analyze(&[]).expect("parse");
        assert_eq!(opts.provider, ExecutionProviderKind::Auto);
        assert_eq!(opts.batch_size.get(), DEFAULT_BATCH_SIZE);
        assert!(opts.cuda.is_unset());
        assert_eq!(opts, InferenceOptions::from(ExecutionProviderKind::Auto));
    }

    #[test]
    fn options_map_to_inference_options() {
        let opts = parse_analyze(&[
            "--provider",
            "cuda",
            "--batch-size",
            "64",
            "--cuda-tf32",
            "OFF",
            "--cuda-graph",
            "on",
        ])
        .expect("parse");
        assert_eq!(opts.provider, ExecutionProviderKind::Cuda);
        assert_eq!(opts.batch_size.get(), 64);
        assert_eq!(
            opts.cuda,
            CudaTuning {
                tf32: Some(false),
                conv1d_pad_to_nc1d: None,
                cuda_graph: Some(true),
            }
        );
    }

    #[test]
    fn invalid_values_are_value_validation_errors() {
        for extra in [
            ["--batch-size", "0"],
            ["--batch-size", "257"],
            ["--cuda-tf32", "yes"],
            ["--cuda-conv1d-pad-to-nc1d", "2"],
            ["--cuda-graph", "enable"],
        ] {
            let err = parse_analyze(&extra).expect_err("must reject");
            assert_eq!(
                err.kind(),
                clap::error::ErrorKind::ValueValidation,
                "{extra:?}"
            );
        }
    }
}

#[cfg(test)]
mod cli_compare_accel_tests {
    use super::*;
    use holter_analysis_assist::accel_compare::report::{
        assemble_report, ConfigInfo, Verdict, EXIT_SUCCESS, EXIT_THRESHOLD_FAILED,
    };

    fn parse_compare(extra: &[&str]) -> Result<CompareConfig, clap::Error> {
        let mut argv = vec!["holter-analysis-assist", "compare-accel", "a.ecl"];
        argv.extend_from_slice(extra);
        match Cli::try_parse_from(argv)?.command {
            Commands::CompareAccel(args) => Ok(args.into_config()),
            other => panic!("unexpected subcommand {other:?}"),
        }
    }

    #[test]
    fn omitted_options_use_design_defaults() {
        let cfg = parse_compare(&["b.ecl"]).expect("parse");
        assert_eq!(
            cfg.ecl_paths,
            vec![PathBuf::from("a.ecl"), PathBuf::from("b.ecl")]
        );
        assert_eq!(cfg.model, resolve_model_source(None));
        assert_eq!(
            cfg.candidate,
            InferenceOptions::from(ExecutionProviderKind::Auto)
        );
        assert_eq!(cfg.tolerance_samples, 40);
        assert_eq!(cfg.prob_stride, 1);
        assert_eq!(cfg.max_windows, None);
        assert_eq!(cfg.report_dir, PathBuf::from("output/accel_compare"));
        assert!(cfg.thresholds.is_empty());
    }

    #[test]
    fn options_map_to_compare_config() {
        let cfg = parse_compare(&[
            "--model",
            "m.onnx",
            "--provider",
            "cuda",
            "--batch-size",
            "32",
            "--cuda-graph",
            "on",
            "--tolerance-samples",
            "10",
            "--prob-stride",
            "8",
            "--max-windows",
            "100",
            "--report-dir",
            "out/cmp",
            "--min-rhythm-window-agreement",
            "0.99",
            "--min-beat-match-rate",
            "0.98",
            "--min-beat-class-agreement",
            "0.97",
            "--max-prob-abs-diff",
            "0.01",
            "--max-offset-samples",
            "2",
        ])
        .expect("parse");
        assert_eq!(cfg.model, ModelSource::Path(PathBuf::from("m.onnx")));
        assert_eq!(cfg.candidate.provider, ExecutionProviderKind::Cuda);
        assert_eq!(cfg.candidate.batch_size.get(), 32);
        assert_eq!(cfg.candidate.cuda.cuda_graph, Some(true));
        assert_eq!(cfg.tolerance_samples, 10);
        assert_eq!(cfg.prob_stride, 8);
        assert_eq!(cfg.max_windows, Some(100));
        assert_eq!(cfg.report_dir, PathBuf::from("out/cmp"));
        assert_eq!(
            cfg.thresholds,
            CompareThresholds {
                min_rhythm_window_agreement: Some(0.99),
                min_beat_match_rate: Some(0.98),
                min_beat_class_agreement: Some(0.97),
                max_prob_abs_diff: Some(0.01),
                max_offset_samples: Some(2.0),
            }
        );
    }

    #[test]
    fn invalid_values_are_value_validation_errors() {
        for extra in [
            ["--prob-stride", "0"],
            ["--max-windows", "0"],
            ["--tolerance-samples", "-1"],
            ["--min-beat-match-rate", "1.0001"],
            ["--min-rhythm-window-agreement", "-0.01"],
            ["--min-beat-class-agreement", "nan"],
            ["--max-prob-abs-diff", "NaN"],
            ["--max-prob-abs-diff", "-0.001"],
            ["--max-prob-abs-diff", "inf"],
            ["--max-offset-samples", "inf"],
            ["--max-offset-samples", "-0.5"],
        ] {
            let err = parse_compare(&extra).expect_err("must reject");
            assert_eq!(
                err.kind(),
                clap::error::ErrorKind::ValueValidation,
                "{extra:?}"
            );
        }
    }

    #[test]
    fn exit_status_follows_verdict() {
        let cpu = InferenceOptions::from(ExecutionProviderKind::Cpu);
        let mut report = assemble_report(
            ConfigInfo::new(&cpu, None),
            ConfigInfo::new(&cpu, None),
            DEFAULT_TOLERANCE_SAMPLES,
            Vec::new(),
            &CompareThresholds::default(),
        );
        assert_eq!(i32::from(compare_exit_status(&report)), EXIT_SUCCESS);
        report.verdict = Some(Verdict {
            checks: Vec::new(),
            passed: true,
        });
        assert_eq!(i32::from(compare_exit_status(&report)), EXIT_SUCCESS);
        report.verdict = Some(Verdict {
            checks: Vec::new(),
            passed: false,
        });
        assert_eq!(
            i32::from(compare_exit_status(&report)),
            EXIT_THRESHOLD_FAILED
        );
        assert_eq!(compare_exit_status(&report), 2);
    }
}

#[cfg(test)]
mod cli_model_select_tests {
    use super::*;

    #[test]
    fn specified_model_always_path() {
        let path = PathBuf::from("/tmp/custom.onnx");
        let src = resolve_model_source(Some(path.clone()));
        assert_eq!(src, ModelSource::Path(path));
    }

    #[cfg(not(feature = "embedded-model"))]
    #[test]
    fn unspecified_without_feature_uses_dev_default_path() {
        let src = resolve_model_source(None);
        assert_eq!(
            src,
            ModelSource::Path(PathBuf::from(DEFAULT_DEV_MODEL)),
            "non-embedded build must keep development default path"
        );
    }

    #[cfg(feature = "embedded-model")]
    #[test]
    fn unspecified_with_feature_uses_embedded() {
        let src = resolve_model_source(None);
        assert_eq!(
            src,
            ModelSource::Embedded,
            "embedded-model build must default to Embedded when --model omitted"
        );
    }

    #[cfg(feature = "embedded-model")]
    #[test]
    fn specified_model_overrides_embedded_default() {
        let path = PathBuf::from("resources/models/override.onnx");
        let src = resolve_model_source(Some(path.clone()));
        assert_eq!(
            src,
            ModelSource::Path(path),
            "--model must always win over Embedded default"
        );
    }
}
