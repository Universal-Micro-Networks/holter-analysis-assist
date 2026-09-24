#!/usr/bin/env python3
"""Compare BeatSense Python reference vs Rust analyze-ecl on the same ECL.

Reports:
1. Preprocess shape / window-grid agreement
2. Keras vs ONNX Runtime numeric drift on the first N windows
3. Final beat CSV agreement (position match within ±80 ms, class/rhythm)

Usage (repo root)::

    source .venv-export/bin/activate
    pip install -r tools/export/requirements.txt pandas scipy
    PYTHONPATH=tools python tools/compare/compare_pipelines.py \\
        --ecl /path/to/file.ecl \\
        --weights resources/models/phase2_....weights.h5 \\
        --onnx resources/models/phase2_rev1.onnx \\
        --max-windows 20
"""

from __future__ import annotations

import argparse
import json
import os
import subprocess
import sys
from pathlib import Path

import numpy as np
import pandas as pd

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
    p.add_argument("--max-windows", type=int, default=20)
    p.add_argument("--outdir", type=Path, default=REPO / "output/compare")
    p.add_argument(
        "--rust-bin",
        type=Path,
        default=None,
        help="Optional path to release binary; otherwise cargo run --release",
    )
    p.add_argument(
        "--provider",
        default="cpu",
        choices=["auto", "cpu", "cuda", "coreml"],
        help="Rust ONNX execution provider",
    )
    return p.parse_args()


def python_limited_analyze(
    ecl: Path,
    weights: Path,
    onnx: Path,
    max_windows: int,
    out_csv: Path,
) -> dict:
    sys.path.insert(0, str(REPO / "tools"))
    from beatsense.model import WINDOW_SAMPLES, load_phase2_model
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

    source_info = parse_ecl_filename(ecl)
    ecg_all_250, _ = read_ecl_adc_counts(ecl)
    ecg_valid_500, starts_abs_500, timeline_start_abs, timeline_end_abs = (
        build_ai_continuous_signal(ecg_all_250, source_info)
    )
    n_windows_full = int(len(starts_abs_500))
    starts_abs_500 = np.asarray(starts_abs_500[:max_windows])

    _full, inference_model, weight_sha = load_phase2_model(weights)
    sess = ort.InferenceSession(str(onnx), providers=["CPUExecutionProvider"])
    inp_name = sess.get_inputs()[0].name

    all_candidates = []
    rhythm_windows = []
    keras_vs_ort = []

    # batch_size=1 to align with Rust/ONNX fixed batch
    for offset, abs_batch, x in iter_ai_batches(
        ecg_valid_500, starts_abs_500, timeline_start_abs, batch_size=1
    ):
        pred_raw = inference_model(x, training=False)
        if not isinstance(pred_raw, dict):
            pred_raw = {
                name: value
                for name, value in zip(inference_model.output_names, pred_raw)
            }
        beat = np.asarray(pred_raw["beat"], dtype=np.float32)
        event = np.asarray(pred_raw["event"], dtype=np.float32)
        rhythm = np.asarray(pred_raw["rhythm"], dtype=np.float32)

        ort_outs = sess.run(None, {inp_name: x})
        keras_vs_ort.append(
            {
                "window": int(offset),
                "beat_max_abs": float(np.max(np.abs(beat - ort_outs[0]))),
                "event_max_abs": float(np.max(np.abs(event - ort_outs[1]))),
                "rhythm_max_abs": float(np.max(np.abs(rhythm.reshape(-1) - ort_outs[2].reshape(-1)))),
            }
        )

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

    return {
        "weight_sha256": weight_sha,
        "n_samples_500": int(len(ecg_valid_500)),
        "n_windows_full_before_truncate": n_windows_full,
        "n_windows_used": int(len(starts_abs_500)),
        "first_start_abs": int(starts_abs_500[0]) if len(starts_abs_500) else None,
        "last_start_abs": int(starts_abs_500[-1]) if len(starts_abs_500) else None,
        "timeline_start_abs": int(timeline_start_abs),
        "timeline_end_abs": int(timeline_end_abs),
        "signal_mean": float(np.mean(ecg_valid_500)),
        "signal_std": float(np.std(ecg_valid_500)),
        "signal_head": ecg_valid_500[:8].astype(float).tolist(),
        "keras_vs_ort": keras_vs_ort,
        "n_beats": int(len(export_df)),
        "unknown_median": float(unknown_median),
        "unknown_threshold": float(unknown_threshold),
        "csv": str(out_csv),
    }


def run_rust(
    ecl: Path,
    onnx: Path,
    out_csv: Path,
    max_windows: int,
    rust_bin: Path | None,
    provider: str = "cpu",
) -> None:
    out_csv.parent.mkdir(parents=True, exist_ok=True)
    if rust_bin is not None:
        cmd = [
            str(rust_bin),
            "analyze-ecl",
            str(ecl),
            "--model",
            str(onnx),
            "--output",
            str(out_csv),
            "--max-windows",
            str(max_windows),
            "--provider",
            provider,
        ]
    else:
        cmd = [
            "cargo",
            "run",
            "--release",
            "--",
            "analyze-ecl",
            str(ecl),
            "--model",
            str(onnx),
            "--output",
            str(out_csv),
            "--max-windows",
            str(max_windows),
            "--provider",
            provider,
        ]
    env = dict(os.environ)
    env["CARGO_HOME"] = str(REPO / ".cargo-tools")
    env["RUSTUP_HOME"] = str(REPO / ".rustup-tools")
    cargo_bin = str(REPO / ".cargo-tools" / "bin")
    user_cargo = str(Path.home() / ".cargo" / "bin")
    env["PATH"] = (
        cargo_bin
        + os.pathsep
        + user_cargo
        + os.pathsep
        + env.get("PATH", "")
    )
    env["CARGO_TARGET_DIR"] = str(REPO / "target")
    print("Running:", " ".join(cmd))
    subprocess.run(cmd, cwd=REPO, check=True, env=env)


def match_beats(py: pd.DataFrame, rs: pd.DataFrame, tol_samples: int = 40) -> dict:
    """Match beats by absolute sample inferred from beat_time + record grid.

    We parse beat_time and also rely on ordered nearest-neighbour within ±tol.
    """
    def to_ns(series: pd.Series) -> np.ndarray:
        return pd.to_datetime(series).astype("int64").to_numpy()

    py_t = to_ns(py["beat_time"])
    rs_t = to_ns(rs["beat_time"])
    # 500 Hz → 2 ms/sample; 40 samples = 80 ms
    tol_ns = int(tol_samples * (1e9 / 500.0))

    used_rs = set()
    matches = []
    unmatched_py = 0
    for i, t in enumerate(py_t):
        best_j = None
        best_dt = None
        for j, u in enumerate(rs_t):
            if j in used_rs:
                continue
            dt = abs(int(t) - int(u))
            if dt <= tol_ns and (best_dt is None or dt < best_dt):
                best_dt = dt
                best_j = j
        if best_j is None:
            unmatched_py += 1
            continue
        used_rs.add(best_j)
        matches.append((i, best_j, best_dt))

    unmatched_rs = len(rs) - len(used_rs)
    class_agree = 0
    rhythm_agree = 0
    unknown_agree = 0
    short_agree = 0
    for i, j, _ in matches:
        if str(py.iloc[i]["beat_class"]) == str(rs.iloc[j]["beat_class"]):
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
    }


def main() -> int:
    args = parse_args()
    outdir = args.outdir
    outdir.mkdir(parents=True, exist_ok=True)
    py_csv = outdir / f"python_max{args.max_windows}.csv"
    rs_csv = outdir / f"rust_max{args.max_windows}.csv"

    print("=" * 72)
    print("1) Python limited analyze + Keras/ORT drift")
    print("=" * 72)
    py_meta = python_limited_analyze(
        args.ecl, args.weights, args.onnx, args.max_windows, py_csv
    )

    print("=" * 72)
    print("2) Rust analyze-ecl")
    print("=" * 72)
    run_rust(
        args.ecl,
        args.onnx,
        rs_csv,
        args.max_windows,
        args.rust_bin,
        provider=args.provider,
    )

    py_df = pd.read_csv(py_csv)
    rs_df = pd.read_csv(rs_csv)
    beat_cmp = match_beats(py_df, rs_df, tol_samples=40)

    ort_stats = py_meta["keras_vs_ort"]
    report = {
        "ecl": str(args.ecl),
        "max_windows": args.max_windows,
        "preprocess_python": {
            k: py_meta[k]
            for k in (
                "n_samples_500",
                "n_windows_used",
                "first_start_abs",
                "last_start_abs",
                "timeline_start_abs",
                "timeline_end_abs",
                "signal_mean",
                "signal_std",
                "signal_head",
                "unknown_median",
                "unknown_threshold",
            )
        },
        "keras_vs_ort": {
            "n_windows": len(ort_stats),
            "beat_max_abs_max": max(x["beat_max_abs"] for x in ort_stats) if ort_stats else None,
            "event_max_abs_max": max(x["event_max_abs"] for x in ort_stats) if ort_stats else None,
            "rhythm_max_abs_max": max(x["rhythm_max_abs"] for x in ort_stats) if ort_stats else None,
            "beat_max_abs_mean": float(np.mean([x["beat_max_abs"] for x in ort_stats])) if ort_stats else None,
        },
        "beat_csv_compare": beat_cmp,
        "python_csv": str(py_csv),
        "rust_csv": str(rs_csv),
    }
    report_path = outdir / f"report_max{args.max_windows}.json"
    report_path.write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")

    print("=" * 72)
    print("SUMMARY")
    print("=" * 72)
    print(json.dumps(report["keras_vs_ort"], indent=2))
    print(json.dumps(report["beat_csv_compare"], indent=2))
    print("wrote", report_path)
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
