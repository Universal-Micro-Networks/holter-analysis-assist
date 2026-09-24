//! Per-window ONNX inference microbench (Rust ort).
//!
//! Usage:
//!   cargo run --release --example bench_infer -- \
//!     resources/models/phase2_rev1.onnx /path/to/windows cpu|coreml [warmup]
use holter_analysis_assist::phase2::{ExecutionProviderKind, Phase2Model, WINDOW_SAMPLES};
use std::env;
use std::fs;
use std::path::PathBuf;
use std::str::FromStr;
use std::time::Instant;

fn main() {
    let mut args = env::args().skip(1);
    let onnx = PathBuf::from(args.next().expect("onnx path"));
    let win_dir = PathBuf::from(args.next().expect("window dir"));
    let provider = ExecutionProviderKind::from_str(
        &args.next().unwrap_or_else(|| "cpu".into()),
    )
    .expect("provider");
    let warmup: usize = args
        .next()
        .unwrap_or_else(|| "3".into())
        .parse()
        .expect("warmup");

    let mut files: Vec<_> = fs::read_dir(&win_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "f32").unwrap_or(false))
        .collect();
    files.sort();

    eprintln!("loading model with provider={provider} ...");
    let load_t0 = Instant::now();
    let mut model = Phase2Model::load_with_provider(&onnx, provider).expect("load onnx");
    let load_ms = load_t0.elapsed().as_secs_f64() * 1000.0;
    eprintln!("model loaded in {load_ms:.1} ms");

    let mut windows = Vec::new();
    for f in &files {
        let bytes = fs::read(f).unwrap();
        assert_eq!(bytes.len(), WINDOW_SAMPLES * 4);
        let mut v = Vec::with_capacity(WINDOW_SAMPLES);
        for c in bytes.chunks_exact(4) {
            v.push(f32::from_le_bytes([c[0], c[1], c[2], c[3]]));
        }
        windows.push(v);
    }

    for w in windows.iter().take(warmup) {
        let _ = model.infer_window(w).unwrap();
    }

    let mut times_ms = Vec::with_capacity(windows.len());
    for w in &windows {
        let t0 = Instant::now();
        let _ = model.infer_window(w).unwrap();
        times_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
    }

    let n = times_ms.len() as f64;
    let mean = times_ms.iter().sum::<f64>() / n;
    let mut sorted = times_ms.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sorted[sorted.len() / 2];
    let p95_idx = ((sorted.len() as f64) * 0.95) as usize;
    let p95 = sorted[p95_idx.min(sorted.len() - 1)];
    println!(
        "{{ \"backend\": \"ort_rust\", \"provider\": \"{}\", \"load_ms\": {:.6}, \"n\": {}, \"mean_ms\": {:.6}, \"median_ms\": {:.6}, \"p95_ms\": {:.6}, \"min_ms\": {:.6}, \"max_ms\": {:.6}, \"total_s\": {:.6}, \"throughput_windows_per_s\": {:.6} }}",
        provider.as_str(),
        load_ms,
        times_ms.len(),
        mean,
        median,
        p95,
        sorted[0],
        sorted[sorted.len() - 1],
        times_ms.iter().sum::<f64>() / 1000.0,
        1000.0 / mean
    );
}
