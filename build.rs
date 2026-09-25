//! EmbedBuildGate: validate and stage model bytes when `embedded-model` is enabled.
//!
//! When the feature is off, this build script is a no-op so path-only development
//! builds succeed without `HOLTER_EMBEDDED_MODEL_PATH`.
//! When the feature is on, the env var must point to a readable, non-empty file;
//! bytes are copied to `OUT_DIR/embedded_model.bin` for later `include_bytes!`.

use std::env;
use std::fs;
use std::path::Path;
use std::process;

fn main() {
    println!("cargo:rerun-if-env-changed=HOLTER_EMBEDDED_MODEL_PATH");

    // Cargo sets CARGO_FEATURE_<NAME> (hyphens → underscores) when the feature is enabled.
    if env::var_os("CARGO_FEATURE_EMBEDDED_MODEL").is_none() {
        return;
    }

    let raw = match env::var("HOLTER_EMBEDDED_MODEL_PATH") {
        Ok(value) => value,
        Err(_) => {
            eprintln!(
                "error: HOLTER_EMBEDDED_MODEL_PATH must be set when building with \
                 --features embedded-model (path to a readable model file)"
            );
            process::exit(1);
        }
    };

    if raw.trim().is_empty() {
        eprintln!(
            "error: HOLTER_EMBEDDED_MODEL_PATH is empty; provide a path to a readable model file"
        );
        process::exit(1);
    }

    let src = Path::new(&raw);
    println!("cargo:rerun-if-changed={}", src.display());

    let meta = match fs::metadata(src) {
        Ok(m) => m,
        Err(err) => {
            eprintln!(
                "error: HOLTER_EMBEDDED_MODEL_PATH is not readable ({}): {}",
                src.display(),
                err
            );
            process::exit(1);
        }
    };

    if !meta.is_file() {
        eprintln!(
            "error: HOLTER_EMBEDDED_MODEL_PATH is not a file: {}",
            src.display()
        );
        process::exit(1);
    }

    if meta.len() == 0 {
        eprintln!(
            "error: HOLTER_EMBEDDED_MODEL_PATH points to an empty file: {}",
            src.display()
        );
        process::exit(1);
    }

    let out_dir = env::var("OUT_DIR").expect("OUT_DIR set by Cargo");
    let dest = Path::new(&out_dir).join("embedded_model.bin");
    if let Err(err) = fs::copy(src, &dest) {
        eprintln!(
            "error: failed to copy model from {} to {}: {}",
            src.display(),
            dest.display(),
            err
        );
        process::exit(1);
    }
}
