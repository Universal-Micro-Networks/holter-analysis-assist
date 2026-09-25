use clap::{Parser, Subcommand, ValueEnum};
use holter_analysis_assist::analyze::analyze_ecl_with_source;
use holter_analysis_assist::license::{
    LicenseConfig, LicenseError, LicenseGate, ReqwestLicenseClient,
};
use holter_analysis_assist::phase2::{self, ExecutionProviderKind, Phase2Model, WINDOW_SAMPLES};
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

        /// ONNX Runtime EP: `auto` (CUDA→CPU), `cuda`, or `cpu`.
        #[arg(long, value_enum, default_value_t = ProviderArg::Auto)]
        provider: ProviderArg,
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

    if let Err(err) = install_and_ensure_startup_licensed(&cli.license_config) {
        // StartupFailed / Config / install errors: identifiable; do not run subcommands.
        eprintln!("error: {err}");
        return ExitCode::FAILURE;
    }

    match cli.command {
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
            provider,
        } => {
            let source = resolve_model_source(model);
            match analyze_ecl_with_source(&ecl, &source, &output, max_windows, provider.into()) {
                Ok((_rows, summary)) => {
                    println!("saved: {}", output.display());
                    println!("beats: {}", summary.beats);
                    println!("windows: {}", summary.windows);
                    println!("Unknown=1: {}", summary.unknown_ones);
                    println!("short_run_flag=1: {}", summary.short_run_ones);
                    ExitCode::SUCCESS
                }
                Err(err) => {
                    eprintln!("error: {err}");
                    ExitCode::FAILURE
                }
            }
        }
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
