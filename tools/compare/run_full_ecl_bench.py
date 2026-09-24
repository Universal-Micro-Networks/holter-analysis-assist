#!/usr/bin/env python3
"""Full-ECL timing + accuracy report (all AI windows).

Runs:
1. Rust analyze-ecl CPU / CUDA (end-to-end) + CSV agreement
2. Python reference vs Rust ONNX (compare_pipelines, all windows)
3. Per-window inference timing: ORT-Python CPU, Rust CPU, Rust CUDA
   (Keras skipped for wall-time; optional --with-keras)
"""
from __future__ import annotations

import argparse
import json
import os
import statistics
import subprocess
import sys
import tempfile
import time
from pathlib import Path

import numpy as np

REPO = Path(__file__).resolve().parents[2]


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument(
        "--ecl",
        type=Path,
        default=REPO / "resources/samples/2501103675_20250512_1415_2359.ecl",
    )
    p.add_argument(
        "--weights",
        type=Path,
        default=REPO
        / "resources/models/phase2_internal_finetuned_eventstrong_noise_20s10s_rev1.weights.h5",
    )
    p.add_argument(
        "--onnx",
        type=Path,
        default=REPO / "resources/models/phase2_rev1.onnx",
    )
    p.add_argument(
        "--rust-bin",
        type=Path,
        default=REPO / "target/release/holter-analysis-assist.exe",
    )
    p.add_argument("--outdir", type=Path, default=REPO / "output/compare_full")
    p.add_argument("--warmup", type=int, default=3)
    p.add_argument(
        "--with-keras",
        action="store_true",
        help="Also bench Keras per-window (slow on full ECL)",
    )
    p.add_argument(
        "--skip-compare-pipelines",
        action="store_true",
        help="Skip Python Keras vs Rust full CSV compare",
    )
    return p.parse_args()


def match_beats(a, b, tol_ms: float = 80.0) -> dict:
    import pandas as pd

    a_t = pd.to_datetime(a["beat_time"]).astype("int64").to_numpy()
    b_t = pd.to_datetime(b["beat_time"]).astype("int64").to_numpy()
    tol_ns = int(tol_ms * 1e6)
    used = set()
    matches = []
    unmatched_a = 0
    for i, t in enumerate(a_t):
        best = None
        best_dt = None
        for j, u in enumerate(b_t):
            if j in used:
                continue
            dt = abs(int(t) - int(u))
            if dt <= tol_ns and (best_dt is None or dt < best_dt):
                best_dt = dt
                best = j
        if best is None:
            unmatched_a += 1
            continue
        used.add(best)
        matches.append((i, best, best_dt))
    unmatched_b = len(b) - len(used)
    n = max(len(matches), 1)

    def rate(key: str) -> float:
        return sum(str(a.iloc[i][key]) == str(b.iloc[j][key]) for i, j, _ in matches) / n

    return {
        "a_beats": int(len(a)),
        "b_beats": int(len(b)),
        "matched": int(len(matches)),
        "unmatched_a": int(unmatched_a),
        "unmatched_b": int(unmatched_b),
        "match_rate_vs_a": float(len(matches) / max(len(a), 1)),
        "class_agree_rate": float(rate("beat_class")),
        "rhythm_agree_rate": float(rate("rhythm_class")),
        "unknown_agree_rate": float(
            sum(int(a.iloc[i]["Unknown"]) == int(b.iloc[j]["Unknown"]) for i, j, _ in matches)
            / n
        ),
        "short_run_agree_rate": float(
            sum(
                int(a.iloc[i]["short_run_flag"]) == int(b.iloc[j]["short_run_flag"])
                for i, j, _ in matches
            )
            / n
        ),
        "median_abs_dt_ms": float(
            np.median([dt / 1e6 for _, _, dt in matches]) if matches else float("nan")
        ),
        "max_abs_dt_ms": float(
            max((dt / 1e6 for _, _, dt in matches), default=float("nan"))
        ),
    }


def run_analyze(bin_path: Path, ecl: Path, onnx: Path, out_csv: Path, provider: str) -> dict:
    out_csv.parent.mkdir(parents=True, exist_ok=True)
    cmd = [
        str(bin_path),
        "analyze-ecl",
        str(ecl),
        "--model",
        str(onnx),
        "--output",
        str(out_csv),
        "--provider",
        provider,
    ]
    print("Running:", " ".join(cmd), flush=True)
    t0 = time.perf_counter()
    subprocess.run(cmd, cwd=REPO, check=True)
    elapsed = time.perf_counter() - t0
    return {"provider": provider, "elapsed_s": elapsed, "csv": str(out_csv)}


def prepare_windows(ecl: Path):
    sys.path.insert(0, str(REPO / "tools"))
    from beatsense.preprocess import (
        build_ai_continuous_signal,
        iter_ai_batches,
        parse_ecl_filename,
        read_ecl_adc_counts,
    )

    info = parse_ecl_filename(ecl)
    ecg, _ = read_ecl_adc_counts(ecl)
    ecg500, starts, t0, _t1 = build_ai_continuous_signal(ecg, info)
    windows = []
    for _offset, _abs_batch, x in iter_ai_batches(ecg500, starts, t0, batch_size=1):
        windows.append(np.asarray(x, dtype=np.float32).copy())
    return windows, int(len(starts))


def summarize(xs: list[float]) -> dict:
    return {
        "n": len(xs),
        "mean_ms": float(statistics.fmean(xs)) if xs else None,
        "median_ms": float(statistics.median(xs)) if xs else None,
        "p95_ms": float(sorted(xs)[int(round((len(xs) - 1) * 0.95))]) if xs else None,
        "min_ms": float(min(xs)) if xs else None,
        "max_ms": float(max(xs)) if xs else None,
        "total_s": float(sum(xs) / 1000.0) if xs else None,
        "throughput_windows_per_s": float(1000.0 / statistics.fmean(xs)) if xs else None,
    }


def bench_ort_python(onnx: Path, windows: list[np.ndarray], warmup: int) -> dict:
    import onnxruntime as ort

    sess = ort.InferenceSession(str(onnx), providers=["CPUExecutionProvider"])
    name = sess.get_inputs()[0].name
    for w in windows[:warmup]:
        sess.run(None, {name: w})
    times = []
    for w in windows:
        t0 = time.perf_counter()
        sess.run(None, {name: w})
        times.append((time.perf_counter() - t0) * 1000.0)
    return {"backend": "onnxruntime_python_cpu", **summarize(times)}


def bench_keras(weights: Path, windows: list[np.ndarray], warmup: int) -> dict:
    sys.path.insert(0, str(REPO / "tools"))
    from beatsense.model import load_phase2_model

    _full, model, sha = load_phase2_model(weights)
    for w in windows[:warmup]:
        out = model(w, training=False)
        if isinstance(out, dict):
            for v in out.values():
                _ = np.asarray(v)
        else:
            for v in out:
                _ = np.asarray(v)
    times = []
    for w in windows:
        t0 = time.perf_counter()
        out = model(w, training=False)
        if isinstance(out, dict):
            for v in out.values():
                _ = np.asarray(v)
        else:
            for v in out:
                _ = np.asarray(v)
        times.append((time.perf_counter() - t0) * 1000.0)
    return {"backend": "keras_tensorflow_cpu", "weights_sha256": sha, **summarize(times)}


def bench_rust(onnx: Path, windows: list[np.ndarray], warmup: int, provider: str) -> dict:
    with tempfile.TemporaryDirectory(prefix="wins_full_") as td:
        td_path = Path(td)
        for i, w in enumerate(windows):
            flat = np.asarray(w, dtype=np.float32).reshape(-1)
            assert flat.size == 10000
            (td_path / f"{i:05d}.f32").write_bytes(flat.astype("<f4").tobytes())

        env = dict(os.environ)
        user_cargo = str(Path.home() / ".cargo" / "bin")
        env["PATH"] = user_cargo + os.pathsep + env.get("PATH", "")
        env["CARGO_TARGET_DIR"] = str(REPO / "target")
        cmd = [
            "cargo",
            "run",
            "--release",
            "--example",
            "bench_infer",
            "--",
            str(onnx),
            str(td_path),
            provider,
            str(warmup),
        ]
        print("Running:", " ".join(cmd), flush=True)
        proc = subprocess.run(
            cmd, cwd=REPO, env=env, check=True, capture_output=True, text=True
        )
        if proc.stderr:
            for ln in proc.stderr.splitlines()[-12:]:
                print(ln)
        lines = [ln for ln in proc.stdout.splitlines() if ln.strip().startswith("{")]
        if not lines:
            raise RuntimeError(proc.stderr or proc.stdout or "no JSON")
        return json.loads(lines[-1])


def main() -> int:
    args = parse_args()
    args.outdir.mkdir(parents=True, exist_ok=True)
    report: dict = {"ecl": str(args.ecl), "outdir": str(args.outdir)}

    print("=" * 72)
    print("A) End-to-end analyze-ecl: Rust CPU vs CUDA (ALL windows)")
    print("=" * 72)
    cpu = run_analyze(
        args.rust_bin, args.ecl, args.onnx, args.outdir / "rust_cpu_full.csv", "cpu"
    )
    cuda = run_analyze(
        args.rust_bin, args.ecl, args.onnx, args.outdir / "rust_cuda_full.csv", "cuda"
    )
    import pandas as pd

    cpu_df = pd.read_csv(cpu["csv"])
    cuda_df = pd.read_csv(cuda["csv"])
    agree = match_beats(cpu_df, cuda_df)
    report["e2e_timing"] = {
        "cpu": cpu,
        "cuda": cuda,
        "speedup_cpu_over_cuda": cpu["elapsed_s"] / cuda["elapsed_s"]
        if cuda["elapsed_s"]
        else None,
    }
    report["e2e_accuracy_cpu_vs_cuda"] = agree
    print(json.dumps(report["e2e_timing"], indent=2))
    print(json.dumps(agree, indent=2))

    if not args.skip_compare_pipelines:
        print("=" * 72)
        print("B) Python Keras reference vs Rust ONNX (ALL windows)")
        print("=" * 72)
        # Discover window count for --max-windows
        windows_probe, n_win = prepare_windows(args.ecl)
        del windows_probe  # free RAM before compare
        cmd = [
            sys.executable,
            str(REPO / "tools/compare/compare_pipelines.py"),
            "--ecl",
            str(args.ecl),
            "--weights",
            str(args.weights),
            "--onnx",
            str(args.onnx),
            "--max-windows",
            str(n_win),
            "--outdir",
            str(args.outdir / "pipelines"),
            "--rust-bin",
            str(args.rust_bin),
            "--provider",
            "cpu",
        ]
        env = dict(os.environ)
        env["PYTHONPATH"] = str(REPO / "tools")
        print("Running:", " ".join(cmd), flush=True)
        subprocess.run(cmd, cwd=REPO, check=True, env=env)
        pipe_report = args.outdir / "pipelines" / f"report_max{n_win}.json"
        report["pipelines_report"] = str(pipe_report)
        if pipe_report.exists():
            report["pipelines"] = json.loads(pipe_report.read_text(encoding="utf-8"))

    print("=" * 72)
    print("C) Per-window inference timing (ALL windows)")
    print("=" * 72)
    windows, n_win = prepare_windows(args.ecl)
    report["n_windows"] = n_win
    print(f"windows={n_win}", flush=True)

    infer = {}
    if args.with_keras:
        print("Benchmark Keras (CPU) ...", flush=True)
        infer["keras"] = bench_keras(args.weights, windows, args.warmup)
        print(json.dumps(infer["keras"], indent=2), flush=True)

    print("Benchmark Python ORT (CPU) ...", flush=True)
    infer["ort_python_cpu"] = bench_ort_python(args.onnx, windows, args.warmup)
    print(json.dumps(infer["ort_python_cpu"], indent=2), flush=True)

    print("Benchmark Rust ort (CPU) ...", flush=True)
    infer["ort_rust_cpu"] = bench_rust(args.onnx, windows, args.warmup, "cpu")
    print(json.dumps(infer["ort_rust_cpu"], indent=2), flush=True)

    print("Benchmark Rust ort (CUDA) ...", flush=True)
    infer["ort_rust_cuda"] = bench_rust(args.onnx, windows, args.warmup, "cuda")
    print(json.dumps(infer["ort_rust_cuda"], indent=2), flush=True)

    def speedup(a: dict | None, b: dict | None) -> float | None:
        if not a or not b or not a.get("mean_ms") or not b.get("mean_ms"):
            return None
        return float(a["mean_ms"] / b["mean_ms"])

    infer["speedup_mean"] = {
        "ort_python_over_rust_cpu": speedup(
            infer.get("ort_python_cpu"), infer.get("ort_rust_cpu")
        ),
        "ort_rust_cpu_over_cuda": speedup(
            infer.get("ort_rust_cpu"), infer.get("ort_rust_cuda")
        ),
        "keras_over_rust_cuda": speedup(infer.get("keras"), infer.get("ort_rust_cuda")),
    }
    report["per_window_inference"] = infer

    out = args.outdir / "full_ecl_report.json"
    out.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print("=" * 72)
    print("DONE", out)
    print(json.dumps({
        "n_windows": report.get("n_windows"),
        "e2e_timing": report.get("e2e_timing"),
        "e2e_accuracy_cpu_vs_cuda": report.get("e2e_accuracy_cpu_vs_cuda"),
        "pipelines_beat_csv_compare": (report.get("pipelines") or {}).get(
            "beat_csv_compare"
        ),
        "pipelines_keras_vs_ort": (report.get("pipelines") or {}).get("keras_vs_ort"),
        "per_window_speedup": infer.get("speedup_mean"),
        "per_window_means_ms": {
            k: (infer[k] or {}).get("mean_ms")
            for k in ("keras", "ort_python_cpu", "ort_rust_cpu", "ort_rust_cuda")
            if k in infer
        },
    }, indent=2))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
