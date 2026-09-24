#!/usr/bin/env python3
"""Full-recording wall-clock + accuracy bench (Python ORT vs Rust CPU).

Runs the available Holter ECL end-to-end (preprocess → infer → postprocess → CSV)
and reports timing plus beat-level agreement (no clinical ground truth; parity vs
Python ONNX Runtime reference).

Usage (repo root)::

    source .venv-export/bin/activate
    PYTHONPATH=tools python tools/compare/bench_fullday.py
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
import time
from pathlib import Path

import numpy as np
import pandas as pd

REPO = Path(__file__).resolve().parents[2]


def parse_args() -> argparse.Namespace:
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument(
        "--ecl",
        type=Path,
        default=None,
        help="ECL path (default: resolve resources/samples/sample.ecl)",
    )
    p.add_argument(
        "--onnx",
        type=Path,
        default=REPO / "resources/models/phase2_rev1.onnx",
    )
    p.add_argument(
        "--weights",
        type=Path,
        default=REPO
        / "resources/models/phase2_internal_finetuned_eventstrong_noise_20s10s_rev1.weights.h5",
        help="Optional Keras weights for small numeric-drift sample",
    )
    p.add_argument("--outdir", type=Path, default=REPO / "output/compare/fullday")
    p.add_argument(
        "--drift-windows",
        type=int,
        default=5,
        help="Windows for Keras vs ORT drift sample (0 to skip)",
    )
    p.add_argument("--skip-python", action="store_true")
    p.add_argument("--skip-rust-cpu", action="store_true")
    p.add_argument(
        "--rust-bin",
        type=Path,
        default=REPO / "target/release/holter-analysis-assist",
    )
    return p.parse_args()


def resolve_ecl(arg: Path | None) -> Path:
    if arg is not None:
        return arg.resolve()
    link = REPO / "resources/samples/sample.ecl"
    return link.resolve()


def cargo_env() -> dict:
    env = dict(os.environ)
    env["CARGO_HOME"] = str(REPO / ".cargo-tools")
    env["RUSTUP_HOME"] = str(REPO / ".rustup-tools")
    env["CARGO_TARGET_DIR"] = str(REPO / "target")
    cargo_bin = str(REPO / ".cargo-tools" / "bin")
    env["PATH"] = cargo_bin + os.pathsep + env.get("PATH", "")
    return env


def ensure_rust_bin(bin_path: Path) -> Path:
    if bin_path.is_file():
        return bin_path
    print("Building release binary ...")
    subprocess.run(
        ["cargo", "build", "--release", "-q"],
        cwd=REPO,
        check=True,
        env=cargo_env(),
    )
    return bin_path


def python_ort_analyze(ecl: Path, onnx: Path, out_csv: Path) -> dict:
    """BeatSense postprocess with ONNX Runtime (CPU) inference — production path parity."""
    sys.path.insert(0, str(REPO / "tools"))
    from beatsense.model import WINDOW_SAMPLES
    from beatsense.preprocess import (
        FS,
        build_ai_continuous_signal,
        iter_ai_batches,
        parse_ecl_filename,
        read_ecl_adc_counts,
    )
    from beatsense.postprocess import (
        build_rhythm_intervals,
        build_unknown_intervals,
        center_best_beats,
        classify_event_sigmoid,
        cluster_candidates,
        complement_intervals,
        detect_run_candidates,
        extract_window_candidates,
        finalize_runs_after_unknown,
        label_points_by_rhythm,
        point_in_intervals,
    )
    import onnxruntime as ort

    t_all = time.perf_counter()

    t0 = time.perf_counter()
    source_info = parse_ecl_filename(ecl)
    ecg_all_250, _ = read_ecl_adc_counts(ecl)
    ecg_valid_500, starts_abs_500, timeline_start_abs, timeline_end_abs = (
        build_ai_continuous_signal(ecg_all_250, source_info)
    )
    starts_abs_500 = np.asarray(starts_abs_500)
    preprocess_s = time.perf_counter() - t0

    t0 = time.perf_counter()
    sess = ort.InferenceSession(str(onnx), providers=["CPUExecutionProvider"])
    inp_name = sess.get_inputs()[0].name
    load_s = time.perf_counter() - t0

    all_candidates = []
    rhythm_windows = []
    infer_times = []
    t0 = time.perf_counter()
    n_win = len(starts_abs_500)
    for i, (offset, abs_batch, x) in enumerate(
        iter_ai_batches(
            ecg_valid_500, starts_abs_500, timeline_start_abs, batch_size=1
        )
    ):
        ti = time.perf_counter()
        beat, event, rhythm = sess.run(None, {inp_name: x})
        infer_times.append((time.perf_counter() - ti) * 1000.0)
        beat = np.asarray(beat, dtype=np.float32)
        event = np.asarray(event, dtype=np.float32)
        rhythm = np.asarray(rhythm, dtype=np.float32)
        all_candidates.extend(
            extract_window_candidates(beat, event, abs_batch, offset)
        )
        rhythm_windows.append(
            {
                "start_sample_500": int(abs_batch[0]),
                "end_sample_500": int(abs_batch[0]) + WINDOW_SAMPLES,
                "rhythm_score": float(rhythm.reshape(-1)[0]),
            }
        )
        if (i + 1) % 200 == 0 or (i + 1) == n_win:
            print(f"  python ORT window {i + 1}/{n_win}", flush=True)
    infer_s = time.perf_counter() - t0

    t0 = time.perf_counter()
    beats = center_best_beats(cluster_candidates(all_candidates))
    beat_df = pd.DataFrame(beats)
    if len(beat_df) == 0:
        beat_df = pd.DataFrame(
            columns=[
                "sample_in_file_500",
                "beat_score",
                "pac_score",
                "pvc_score",
                "n_score",
            ]
        )

    rhythm_intervals, coverage_intervals = build_rhythm_intervals(rhythm_windows)
    beat_positions = beat_df["sample_in_file_500"].to_numpy(dtype=np.int64)
    rhythm_labels = label_points_by_rhythm(beat_positions, rhythm_intervals)
    raw_classes = np.asarray(
        [
            classify_event_sigmoid(pac, pvc)
            for pac, pvc in zip(
                beat_df["pac_score"].to_numpy(dtype=float),
                beat_df["pvc_score"].to_numpy(dtype=float),
            )
        ],
        dtype=object,
    )
    effective = raw_classes.copy()
    effective[(raw_classes == "PAC") & (rhythm_labels == "AF/AFL")] = "PAC_MASKED_AF"
    final_classes = effective.copy()
    final_classes[final_classes == "PAC_MASKED_AF"] = "N"

    af_intervals = [
        (int(x["start"]), int(x["end"]))
        for x in rhythm_intervals
        if x["label"] == "AF/AFL"
    ]
    gap_intervals = complement_intervals(
        timeline_start_abs, timeline_end_abs, coverage_intervals
    )

    beat_time = source_info["study_date"] + pd.to_timedelta(
        beat_positions / FS, unit="s"
    )
    beat_df["timestamp"] = pd.DatetimeIndex(beat_time)
    beat_df["class"] = effective.astype(str)

    run_candidate_df, run_candidate_short_df, _, _ = detect_run_candidates(
        beat_df, af_intervals=af_intervals, gap_intervals=gap_intervals
    )
    unknown_intervals, unknown_median, unknown_threshold = build_unknown_intervals(
        ecg_all_250, source_info, timeline_start_abs, timeline_end_abs
    )
    beat_in_coverage = point_in_intervals(beat_positions, coverage_intervals)
    unknown = (
        point_in_intervals(beat_positions, unknown_intervals) & beat_in_coverage
    ).astype(np.int8)
    _, _, _, accepted_run_member_samples = finalize_runs_after_unknown(
        run_candidate_df, run_candidate_short_df, unknown_intervals
    )
    short_run_flag = np.asarray(
        [1 if int(p) in accepted_run_member_samples else 0 for p in beat_positions],
        dtype=np.int8,
    )
    short_run_flag = np.where(unknown == 1, 0, short_run_flag)

    beat_time_text = (
        pd.DatetimeIndex(beat_time).strftime("%Y-%m-%d %H:%M:%S.%f").str[:-3]
    )
    export_df = pd.DataFrame(
        {
            "record_id": [ecl.stem] * len(beat_df),
            "beat_idx": np.arange(len(beat_df), dtype=np.int64),
            "beat_time": beat_time_text,
            "Unknown": unknown,
            "beat_class": final_classes.astype(str),
            "rhythm_class": rhythm_labels.astype(str),
            "short_run_flag": short_run_flag,
        }
    )
    out_csv.parent.mkdir(parents=True, exist_ok=True)
    export_df.to_csv(out_csv, index=False, encoding="utf-8-sig")
    post_s = time.perf_counter() - t0
    total_s = time.perf_counter() - t_all

    duration_h = len(ecg_valid_500) / FS / 3600.0
    return {
        "backend": "python_onnxruntime_cpu",
        "csv": str(out_csv),
        "n_windows": int(n_win),
        "n_beats": int(len(export_df)),
        "n_samples_500": int(len(ecg_valid_500)),
        "duration_h": float(duration_h),
        "timings_s": {
            "preprocess": preprocess_s,
            "model_load": load_s,
            "inference_loop": infer_s,
            "postprocess_export": post_s,
            "total_wall": total_s,
        },
        "infer_ms": {
            "mean": float(np.mean(infer_times)) if infer_times else None,
            "median": float(np.median(infer_times)) if infer_times else None,
            "p95": float(np.percentile(infer_times, 95)) if infer_times else None,
        },
        "unknown_median": float(unknown_median),
        "unknown_threshold": float(unknown_threshold),
        "class_counts": export_df["beat_class"].value_counts().to_dict(),
        "rhythm_counts": export_df["rhythm_class"].value_counts().to_dict(),
    }


def keras_ort_drift_sample(
    ecl: Path, weights: Path, onnx: Path, n_windows: int
) -> dict | None:
    if n_windows <= 0 or not weights.is_file():
        return None
    sys.path.insert(0, str(REPO / "tools"))
    from beatsense.model import load_phase2_model
    from beatsense.preprocess import (
        build_ai_continuous_signal,
        iter_ai_batches,
        parse_ecl_filename,
        read_ecl_adc_counts,
    )
    import onnxruntime as ort

    info = parse_ecl_filename(ecl)
    ecg, _ = read_ecl_adc_counts(ecl)
    ecg500, starts, t0, _ = build_ai_continuous_signal(ecg, info)
    starts = np.asarray(starts[:n_windows])
    _full, model, sha = load_phase2_model(weights)
    sess = ort.InferenceSession(str(onnx), providers=["CPUExecutionProvider"])
    inp = sess.get_inputs()[0].name
    rows = []
    for offset, _abs, x in iter_ai_batches(ecg500, starts, t0, batch_size=1):
        pred = model(x, training=False)
        if not isinstance(pred, dict):
            pred = {n: v for n, v in zip(model.output_names, pred)}
        beat = np.asarray(pred["beat"], dtype=np.float32)
        event = np.asarray(pred["event"], dtype=np.float32)
        rhythm = np.asarray(pred["rhythm"], dtype=np.float32)
        o0, o1, o2 = sess.run(None, {inp: x})
        rows.append(
            {
                "window": int(offset),
                "beat_max_abs": float(np.max(np.abs(beat - o0))),
                "event_max_abs": float(np.max(np.abs(event - o1))),
                "rhythm_max_abs": float(
                    np.max(np.abs(rhythm.reshape(-1) - np.asarray(o2).reshape(-1)))
                ),
            }
        )
    return {
        "weight_sha256": sha,
        "n_windows": len(rows),
        "beat_max_abs_max": max(r["beat_max_abs"] for r in rows),
        "event_max_abs_max": max(r["event_max_abs"] for r in rows),
        "rhythm_max_abs_max": max(r["rhythm_max_abs"] for r in rows),
        "beat_max_abs_mean": float(np.mean([r["beat_max_abs"] for r in rows])),
    }


def run_rust(
    ecl: Path,
    onnx: Path,
    out_csv: Path,
    provider: str,
    rust_bin: Path,
) -> dict:
    out_csv.parent.mkdir(parents=True, exist_ok=True)
    cmd = [
        str(rust_bin),
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
    proc = subprocess.run(
        cmd,
        cwd=REPO,
        env=cargo_env(),
        check=True,
        capture_output=True,
        text=True,
    )
    wall = time.perf_counter() - t0
    # Parse "beats: N" from stdout
    n_beats = None
    for ln in (proc.stdout or "").splitlines():
        if ln.startswith("beats:"):
            try:
                n_beats = int(ln.split(":", 1)[1].strip())
            except ValueError:
                pass
        print(ln)
    for ln in (proc.stderr or "").splitlines():
        print(ln)
    df = pd.read_csv(out_csv)
    return {
        "backend": f"rust_ort_{provider}",
        "provider": provider,
        "csv": str(out_csv),
        "n_beats": int(n_beats if n_beats is not None else len(df)),
        "timings_s": {"total_wall": wall},
        "class_counts": df["beat_class"].value_counts().to_dict(),
        "rhythm_counts": df["rhythm_class"].value_counts().to_dict(),
        "stderr_tail": (proc.stderr or "").splitlines()[-12:],
    }


def match_beats(py: pd.DataFrame, rs: pd.DataFrame, tol_samples: int = 40) -> dict:
    """Sorted two-pointer match within ±tol samples (80 ms @ 500 Hz)."""
    py_t = pd.to_datetime(py["beat_time"]).astype("int64").to_numpy()
    rs_t = pd.to_datetime(rs["beat_time"]).astype("int64").to_numpy()
    tol_ns = int(tol_samples * (1e9 / 500.0))

    order_py = np.argsort(py_t)
    order_rs = np.argsort(rs_t)
    py_t_s = py_t[order_py]
    rs_t_s = rs_t[order_rs]

    matches = []  # (py_orig_i, rs_orig_j, dt_ns)
    used_rs = np.zeros(len(rs_t_s), dtype=bool)
    j0 = 0
    for ii, t in enumerate(py_t_s):
        while j0 < len(rs_t_s) and rs_t_s[j0] < t - tol_ns:
            j0 += 1
        best_j = None
        best_dt = None
        j = j0
        while j < len(rs_t_s) and rs_t_s[j] <= t + tol_ns:
            if not used_rs[j]:
                dt = abs(int(t) - int(rs_t_s[j]))
                if best_dt is None or dt < best_dt:
                    best_dt = dt
                    best_j = j
            j += 1
        if best_j is None:
            continue
        used_rs[best_j] = True
        matches.append((int(order_py[ii]), int(order_rs[best_j]), int(best_dt)))

    unmatched_py = len(py) - len(matches)
    unmatched_rs = int((~used_rs).sum())
    class_agree = rhythm_agree = unknown_agree = short_agree = 0
    class_conf: dict[str, dict[str, int]] = {}
    for i, j, _ in matches:
        pc = str(py.iloc[i]["beat_class"])
        rc = str(rs.iloc[j]["beat_class"])
        class_conf.setdefault(pc, {})
        class_conf[pc][rc] = class_conf[pc].get(rc, 0) + 1
        if pc == rc:
            class_agree += 1
        if str(py.iloc[i]["rhythm_class"]) == str(rs.iloc[j]["rhythm_class"]):
            rhythm_agree += 1
        if int(py.iloc[i]["Unknown"]) == int(rs.iloc[j]["Unknown"]):
            unknown_agree += 1
        if int(py.iloc[i]["short_run_flag"]) == int(rs.iloc[j]["short_run_flag"]):
            short_agree += 1

    n = max(len(matches), 1)
    return {
        "py_beats": int(len(py)),
        "rs_beats": int(len(rs)),
        "matched": int(len(matches)),
        "unmatched_py": int(unmatched_py),
        "unmatched_rs": int(unmatched_rs),
        "match_rate_vs_py": float(len(matches) / max(len(py), 1)),
        "match_rate_vs_rs": float(len(matches) / max(len(rs), 1)),
        "class_agree_rate": float(class_agree / n),
        "rhythm_agree_rate": float(rhythm_agree / n),
        "unknown_agree_rate": float(unknown_agree / n),
        "short_run_agree_rate": float(short_agree / n),
        "median_abs_dt_ms": float(
            np.median([dt / 1e6 for _, _, dt in matches]) if matches else float("nan")
        ),
        "max_abs_dt_ms": float(
            max((dt / 1e6 for _, _, dt in matches), default=float("nan"))
        ),
        "class_confusion_py_vs_rs": class_conf,
    }


def main() -> int:
    args = parse_args()
    ecl = resolve_ecl(args.ecl)
    outdir = args.outdir
    outdir.mkdir(parents=True, exist_ok=True)

    print(f"ECL: {ecl}")
    print(f"size: {ecl.stat().st_size / 1e6:.1f} MB")
    print(f"onnx: {args.onnx}")

    report: dict = {
        "ecl": str(ecl),
        "ecl_name": ecl.name,
        "onnx": str(args.onnx),
        "note": (
            "Accuracy = beat-level parity vs Python ONNX Runtime reference "
            "(no clinical ground-truth labels). Recording is the available Holter ECL "
            "(~scheduled span from filename; actual AI-valid duration from preprocess)."
        ),
    }

    if not args.skip_python:
        print("=" * 72)
        print("Python ORT full analyze")
        print("=" * 72)
        py = python_ort_analyze(ecl, args.onnx, outdir / "python_ort_fullday.csv")
        report["python_ort"] = py
        print(json.dumps({k: py[k] for k in ("n_windows", "n_beats", "duration_h", "timings_s", "infer_ms")}, indent=2))

        print("=" * 72)
        print(f"Keras vs ORT drift sample (n={args.drift_windows})")
        print("=" * 72)
        drift = keras_ort_drift_sample(ecl, args.weights, args.onnx, args.drift_windows)
        report["keras_vs_ort_sample"] = drift
        print(json.dumps(drift, indent=2))

    rust_bin = ensure_rust_bin(args.rust_bin)
    py_csv = outdir / "python_ort_fullday.csv"

    if not args.skip_rust_cpu:
        print("=" * 72)
        print("Rust ORT CPU full analyze")
        print("=" * 72)
        rs_cpu = run_rust(ecl, args.onnx, outdir / "rust_cpu_fullday.csv", "cpu", rust_bin)
        report["rust_cpu"] = rs_cpu
        print(json.dumps(rs_cpu["timings_s"], indent=2))
        if py_csv.is_file():
            cmp_cpu = match_beats(pd.read_csv(py_csv), pd.read_csv(rs_cpu["csv"]))
            report["accuracy_python_vs_rust_cpu"] = cmp_cpu
            print(json.dumps(cmp_cpu, indent=2))

    # Speed summary
    speed = {}
    for key in ("python_ort", "rust_cpu"):
        if key in report and "timings_s" in report[key]:
            speed[key] = report[key]["timings_s"].get("total_wall")
    if speed.get("python_ort") and speed.get("rust_cpu"):
        speed["python_over_rust_cpu"] = speed["python_ort"] / speed["rust_cpu"]
        if report.get("python_ort", {}).get("duration_h"):
            dur_h = float(report["python_ort"]["duration_h"])
            speed["realtime_factor_python"] = dur_h * 3600.0 / speed["python_ort"]
            speed["realtime_factor_rust_cpu"] = dur_h * 3600.0 / speed["rust_cpu"]
    report["speed_summary_s"] = speed

    out = outdir / "report_fullday.json"
    out.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
    print("=" * 72)
    print("SPEED SUMMARY (wall seconds)")
    print(json.dumps(speed, indent=2))
    print("wrote", out)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
