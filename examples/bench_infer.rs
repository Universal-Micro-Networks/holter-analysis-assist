//! Per-window ONNX inference microbench (Rust ort).
//!
//! Usage:
//!   cargo run --release --example bench_infer -- \
//!     resources/models/phase2_rev1.onnx /path/to/windows auto|cpu|cuda [warmup]
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
    let requested = ExecutionProviderKind::from_str(
        &args.next().unwrap_or_else(|| "auto".into()),
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

    eprintln!("loading model with requested_provider={requested} ...");
    let load_t0 = Instant::now();
    let mut model = Phase2Model::load_with_provider(&onnx, requested).expect("load onnx");
    let provider = model.provider();
    let load_ms = load_t0.elapsed().as_secs_f64() * 1000.0;
    eprintln!("model loaded in {load_ms:.1} ms (resolved_provider={provider})");

    let mut times_ms = Vec::with_capacity(files.len());
    for (i, path) in files.iter().enumerate() {
        let bytes = fs::read(path).expect("read window");
        assert_eq!(
            bytes.len(),
            WINDOW_SAMPLES * 4,
            "window {} byte length",
            path.display()
        );
        let mut samples = vec![0.0_f32; WINDOW_SAMPLES];
        for (j, chunk) in bytes.chunks_exact(4).enumerate() {
            samples[j] = f32::from_le_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]);
        }

        if i < warmup {
            let _ = model.infer_window(&samples).expect("warmup infer");
            continue;
        }

        let t0 = Instant::now();
        let _ = model.infer_window(&samples).expect("infer");
        times_ms.push(t0.elapsed().as_secs_f64() * 1000.0);
    }

    let n = times_ms.len();
    let mean = if n == 0 {
        0.0
    } else {
        times_ms.iter().sum::<f64>() / n as f64
    };
    let mut sorted = times_ms.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = if n == 0 {
        0.0
    } else if n % 2 == 1 {
        sorted[n / 2]
    } else {
        (sorted[n / 2 - 1] + sorted[n / 2]) / 2.0
    };
    let p95 = if n == 0 {
        0.0
    } else {
        sorted[((n as f64 - 1.0) * 0.95).round() as usize]
    };
    let min = sorted.first().copied().unwrap_or(0.0);
    let max = sorted.last().copied().unwrap_or(0.0);
    let total_s = times_ms.iter().sum::<f64>() / 1000.0;
    let throughput = if mean > 0.0 { 1000.0 / mean } else { 0.0 };

    println!(
        "{{ \"backend\": \"ort_rust\", \"requested_provider\": \"{}\", \"provider\": \"{}\", \"load_ms\": {:.6}, \"n\": {}, \"mean_ms\": {:.6}, \"median_ms\": {:.6}, \"p95_ms\": {:.6}, \"min_ms\": {:.6}, \"max_ms\": {:.6}, \"total_s\": {:.6}, \"throughput_windows_per_s\": {:.6} }}",
        requested.as_str(),
        provider.as_str(),
        load_ms,
        n,
        mean,
        median,
        p95,
        min,
        max,
        total_s,
        throughput
    );
}
