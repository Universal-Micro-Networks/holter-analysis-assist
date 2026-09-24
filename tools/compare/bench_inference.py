#!/usr/bin/env python3
"""Benchmark per-window inference: Keras vs Python ORT vs Rust ort.

Usage (repo root)::

    source .venv-export/bin/activate
    PYTHONPATH=tools python tools/compare/bench_inference.py \\
      --ecl /path/to.ecl --max-windows 50 --warmup 3
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
        default=Path("/tmp/BeatSense-ecl/BeatSense/input/2501103675_20250512_1415_2359.ecl"),
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
    p.add_argument("--max-windows", type=int, default=50)
    p.add_argument("--warmup", type=int, default=3)
    p.add_argument("--outdir", type=Path, default=REPO / "output/compare")
    return p.parse_args()


def percentile(xs: list[float], p: float) -> float:
    if not xs:
        return float("nan")
    ys = sorted(xs)
    k = (len(ys) - 1) * p / 100.0
    f = int(k)
    c = min(f + 1, len(ys) - 1)
    if f == c:
        return float(ys[f])
    return float(ys[f] + (ys[c] - ys[f]) * (k - f))


def summarize(xs: list[float]) -> dict:
    return {
        "n": len(xs),
        "mean_ms": float(statistics.fmean(xs)) if xs else None,
        "median_ms": float(statistics.median(xs)) if xs else None,
        "p95_ms": float(percentile(xs, 95)),
        "min_ms": float(min(xs)) if xs else None,
        "max_ms": float(max(xs)) if xs else None,
        "total_s": float(sum(xs) / 1000.0) if xs else None,
        "throughput_windows_per_s": float(1000.0 / statistics.fmean(xs)) if xs else None,
    }


def prepare_windows(ecl: Path, max_windows: int):
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
    starts = np.asarray(starts[:max_windows])
    windows = []
    for _offset, _abs_batch, x in iter_ai_batches(ecg500, starts, t0, batch_size=1):
        windows.append(np.asarray(x, dtype=np.float32).copy())
    return windows, int(len(starts))


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
    return {"backend": "keras_tensorflow", "weights_sha256": sha, **summarize(times)}


def bench_ort_python(
    onnx: Path, windows: list[np.ndarray], warmup: int, providers: list[str] | None = None
) -> dict:
    import onnxruntime as ort

    if providers is None:
        providers = ["CPUExecutionProvider"]
    sess = ort.InferenceSession(str(onnx), providers=providers)
    active = sess.get_providers()
    name = sess.get_inputs()[0].name
    for w in windows[:warmup]:
        sess.run(None, {name: w})

    times = []
    for w in windows:
        t0 = time.perf_counter()
        sess.run(None, {name: w})
        times.append((time.perf_counter() - t0) * 1000.0)
    return {
        "backend": "onnxruntime_python",
        "providers": active,
        **summarize(times),
    }


def bench_rust_ort(onnx: Path, windows: list[np.ndarray], warmup: int, provider: str) -> dict:
    with tempfile.TemporaryDirectory(prefix="wins_") as td:
        td_path = Path(td)
        for i, w in enumerate(windows):
            flat = np.asarray(w, dtype=np.float32).reshape(-1)
            assert flat.size == 10000
            (td_path / f"{i:05d}.f32").write_bytes(flat.astype("<f4").tobytes())

        env = dict(os.environ)
        env["CARGO_HOME"] = str(REPO / ".cargo-tools")
        env["RUSTUP_HOME"] = str(REPO / ".rustup-tools")
        cargo_bin = str(REPO / ".cargo-tools" / "bin")
        user_cargo = os.path.expanduser("~/.cargo/bin")
        env["PATH"] = (
            cargo_bin
            + os.pathsep
            + user_cargo
            + os.pathsep
            + env.get("PATH", "")
        )
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
        print("Running:", " ".join(cmd))
        proc = subprocess.run(
            cmd,
            cwd=REPO,
            env=env,
            check=True,
            capture_output=True,
            text=True,
        )
        if proc.stderr:
            # show load timing lines
            for ln in proc.stderr.splitlines()[-8:]:
                print(ln)
        lines = [ln for ln in proc.stdout.splitlines() if ln.strip().startswith("{")]
        if not lines:
            print(proc.stdout)
            print(proc.stderr)
            raise RuntimeError("no JSON from rust bench")
        return json.loads(lines[-1])


def main() -> int:
    args = parse_args()
    args.outdir.mkdir(parents=True, exist_ok=True)

    print("Preparing windows ...")
    windows, n = prepare_windows(args.ecl, args.max_windows)
    print(f"windows={n}, warmup={args.warmup}")

    print("Benchmark Keras ...")
    keras = bench_keras(args.weights, windows, args.warmup)
    print(json.dumps(keras, indent=2))

    print("Benchmark Python ORT (CPU) ...")
    ort_py = bench_ort_python(args.onnx, windows, args.warmup)
    print(json.dumps(ort_py, indent=2))

    print("Benchmark Rust ort (CPU) ...")
    ort_rs_cpu = bench_rust_ort(args.onnx, windows, args.warmup, "cpu")
    print(json.dumps(ort_rs_cpu, indent=2))

    ort_rs_cuda = None
    print("Benchmark Rust ort (CUDA) ...")
    try:
        ort_rs_cuda = bench_rust_ort(args.onnx, windows, args.warmup, "cuda")
        print(json.dumps(ort_rs_cuda, indent=2))
    except Exception as e:
        print(f"CUDA bench failed: {e}")

    ort_rs_coreml = None
    if sys.platform == "darwin":
        print("Benchmark Rust ort (CoreML) ...")
        try:
            ort_rs_coreml = bench_rust_ort(args.onnx, windows, args.warmup, "coreml")
            print(json.dumps(ort_rs_coreml, indent=2))
        except Exception as e:
            print(f"CoreML bench failed: {e}")

    def speedup(a: dict | None, b: dict | None) -> float | None:
        if not a or not b or not a.get("mean_ms") or not b.get("mean_ms"):
            return None
        return float(a["mean_ms"] / b["mean_ms"])

    report = {
        "ecl": str(args.ecl),
        "max_windows": args.max_windows,
        "warmup": args.warmup,
        "note": "Per-window inference only (preprocess excluded).",
        "keras": keras,
        "onnxruntime_python": ort_py,
        "ort_rust_cpu": ort_rs_cpu,
        "ort_rust_cuda": ort_rs_cuda,
        "ort_rust_coreml": ort_rs_coreml,
        "speedup_mean": {
            "keras_over_ort_python": speedup(keras, ort_py),
            "keras_over_ort_rust_cpu": speedup(keras, ort_rs_cpu),
            "keras_over_ort_rust_cuda": speedup(keras, ort_rs_cuda),
            "keras_over_ort_rust_coreml": speedup(keras, ort_rs_coreml),
            "ort_rust_cpu_over_cuda": speedup(ort_rs_cpu, ort_rs_cuda),
            "ort_rust_cpu_over_coreml": speedup(ort_rs_cpu, ort_rs_coreml),
        },
    }
    out = args.outdir / f"bench_infer_w{args.max_windows}.json"
    out.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print("=" * 72)
    print("SPEEDUP (mean_ms ratio; >1 means left is slower)")
    print(json.dumps(report["speedup_mean"], indent=2))
    print("wrote", out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
