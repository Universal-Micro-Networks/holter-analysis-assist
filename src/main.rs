use clap::{Parser, Subcommand, ValueEnum};
use holter_analysis_assist::{classify_ecg, ClassificationResult};
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(
    name = "holter-analysis-assist",
    version,
    about = "Holter ECG arrhythmia classification assist (NORMAL / AF / PAC / PVC)"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
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
}

#[derive(Clone, Copy, Debug, ValueEnum)]
enum OutputFormat {
    Text,
    Json,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command {
        Commands::Classify { input, format } => match classify_ecg(&input) {
            Ok(result) => {
                print_result(&result, format);
                ExitCode::SUCCESS
            }
            Err(err) => {
                eprintln!("error: {err}");
                ExitCode::FAILURE
            }
        },
    }
}

fn print_result(result: &ClassificationResult, format: OutputFormat) {
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
