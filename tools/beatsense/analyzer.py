"""High-level ECL analyzer.

``analyze_ecl()`` connects the three implementation layers:

1. ``preprocess.py``  : ECL -> model input
2. ``model.py``       : build_model -> load weights -> Keras inference
3. ``postprocess.py`` : beat/event/rhythm -> Unknown/RUN -> final CSV

外部利用側は通常このモジュールの ``analyze_ecl`` だけを呼び出します。
"""

from __future__ import annotations

from pathlib import Path

import numpy as np
import pandas as pd

from .model import load_phase2_model, WINDOW_SAMPLES
from .preprocess import (
    FS,
    parse_ecl_filename,
    read_ecl_adc_counts,
    build_ai_continuous_signal,
    iter_ai_batches,
)
from .postprocess import (
    extract_window_candidates,
    cluster_candidates,
    center_best_beats,
    build_rhythm_intervals,
    label_points_by_rhythm,
    classify_event_sigmoid,
    complement_intervals,
    point_in_intervals,
    detect_run_candidates,
    build_unknown_intervals,
    finalize_runs_after_unknown,
)

def analyze_ecl(
    ecl_path: str | Path,
    weights_path: str | Path,
    output_csv: str | Path,
    *,
    batch_size: int = 8,
) -> pd.DataFrame:
    """指定ECLを解析し、BeatSense最終仕様のbeat単位CSVを保存する。"""
    ecl_path = Path(ecl_path)
    weights_path = Path(weights_path)
    output_csv = Path(output_csv)

    source_info = parse_ecl_filename(ecl_path)
    ecg_all_250, _event_code = read_ecl_adc_counts(ecl_path)

    print("[1/6] AI preprocessing ...")
    (
        ecg_valid_500,
        starts_abs_500,
        timeline_start_abs,
        timeline_end_abs,
    ) = build_ai_continuous_signal(ecg_all_250, source_info)

    # Build the exact training-time topology first, then strictly load the H5 weights.
    # The returned inference_model exposes only beat / event / rhythm.
    _full_model, inference_model, weight_sha256 = load_phase2_model(weights_path)

    print(f"[2/6] Keras reference inference: windows={len(starts_abs_500):,}")
    print("      weights SHA-256:", weight_sha256)
    all_candidates: list[dict] = []
    rhythm_windows: list[dict] = []

    for offset, abs_batch, x in iter_ai_batches(
        ecg_valid_500,
        starts_abs_500,
        timeline_start_abs,
        batch_size,
    ):
        pred_raw = inference_model(x, training=False)
        if not isinstance(pred_raw, dict):
            pred_raw = {name: value for name, value in zip(inference_model.output_names, pred_raw)}
        pred = {
            "beat": np.asarray(pred_raw["beat"], dtype=np.float32),
            "event": np.asarray(pred_raw["event"], dtype=np.float32),
            "rhythm": np.asarray(pred_raw["rhythm"], dtype=np.float32),
        }
        all_candidates.extend(
            extract_window_candidates(
                pred["beat"], pred["event"], abs_batch, offset
            )
        )
        rhythm_scores = np.asarray(pred["rhythm"], dtype=np.float32).reshape(-1)
        for i, score in enumerate(rhythm_scores):
            rhythm_windows.append({
                "start_sample_500": int(abs_batch[i]),
                "end_sample_500": int(abs_batch[i]) + WINDOW_SAMPLES,
                "rhythm_score": float(score),
            })

    print("[3/6] Beat / event / rhythm post-processing ...")
    beats = center_best_beats(cluster_candidates(all_candidates))
    beat_df = pd.DataFrame(beats)
    if len(beat_df) == 0:
        beat_df = pd.DataFrame(columns=[
            "sample_in_file_500", "beat_score", "pac_score", "pvc_score", "n_score"
        ])

    rhythm_intervals, coverage_intervals = build_rhythm_intervals(rhythm_windows)
    beat_positions = beat_df["sample_in_file_500"].to_numpy(dtype=np.int64)
    rhythm_labels = label_points_by_rhythm(beat_positions, rhythm_intervals)

    raw_classes = np.asarray([
        classify_event_sigmoid(pac, pvc)
        for pac, pvc in zip(
            beat_df["pac_score"].to_numpy(dtype=float),
            beat_df["pvc_score"].to_numpy(dtype=float),
        )
    ], dtype=object)

    # Reference implementationではraw EVENT classを内部保持し、
    # AF/AFL内のPACだけ effective class = PAC_MASKED_AF とする。
    # customer-facing CSVへ出す際は PAC_MASKED_AF -> N に変換する。
    effective_classes = raw_classes.copy()
    effective_classes[(raw_classes == "PAC") & (rhythm_labels == "AF/AFL")] = "PAC_MASKED_AF"
    final_classes = effective_classes.copy()
    final_classes[final_classes == "PAC_MASKED_AF"] = "N"

    beat_time = source_info["study_date"] + pd.to_timedelta(beat_positions / FS, unit="s")
    beat_df["timestamp"] = pd.DatetimeIndex(beat_time)
    beat_df["class_raw"] = raw_classes.astype(str)
    beat_df["class"] = effective_classes.astype(str)
    beat_df["rhythm_at_beat"] = rhythm_labels.astype(str)

    af_intervals = [
        (int(x["start"]), int(x["end"]))
        for x in rhythm_intervals
        if x["label"] == "AF/AFL"
    ]
    gap_intervals = complement_intervals(
        timeline_start_abs,
        timeline_end_abs,
        coverage_intervals,
    )

    print("[4/6] RRI RUN candidates ...")
    (
        run_candidate_df,
        run_candidate_short_df,
        _run_rr_audit_df,
        _run_reference_summary,
    ) = detect_run_candidates(
        beat_df,
        af_intervals=af_intervals,
        gap_intervals=gap_intervals,
    )

    print("[5/6] Unknown frequency QC ...")
    unknown_intervals, unknown_median, unknown_threshold = build_unknown_intervals(
        ecg_all_250,
        source_info,
        timeline_start_abs,
        timeline_end_abs,
    )

    beat_in_coverage = point_in_intervals(beat_positions, coverage_intervals)
    unknown = (
        point_in_intervals(beat_positions, unknown_intervals)
        & beat_in_coverage
    ).astype(np.int8)

    (
        final_run_df,
        _final_short_df,
        _rejected_run_df,
        accepted_run_member_samples,
    ) = finalize_runs_after_unknown(
        run_candidate_df,
        run_candidate_short_df,
        unknown_intervals,
    )
    # short_run_flagは、FINAL Unknown除外後に採用されたRUNの
    # member RRI endpoint beatだけを1とする。
    short_run_flag = np.asarray(
        [1 if int(p) in accepted_run_member_samples else 0 for p in beat_positions],
        dtype=np.int8,
    )

    print("[6/6] Final CSV ...")
    beat_time_text = pd.DatetimeIndex(beat_time).strftime("%Y-%m-%d %H:%M:%S.%f").str[:-3]

    export_df = pd.DataFrame({
        "record_id": [ecl_path.stem] * len(beat_df),
        "beat_idx": np.arange(len(beat_df), dtype=np.int64),
        "beat_time": beat_time_text,
        "Unknown": unknown,
        "beat_class": final_classes.astype(str),
        "rhythm_class": rhythm_labels.astype(str),
        "short_run_flag": short_run_flag,
    })

    # Final output specificationの整合性チェック。
    if len(export_df):
        if not set(export_df["Unknown"].unique()).issubset({0, 1}):
            raise RuntimeError("Unknown must be binary")
        if not set(export_df["short_run_flag"].unique()).issubset({0, 1}):
            raise RuntimeError("short_run_flag must be binary")
        if np.any((export_df["Unknown"].to_numpy() == 1) & (export_df["short_run_flag"].to_numpy() == 1)):
            raise RuntimeError("Unknown beat cannot have short_run_flag=1")
        if np.any(
            export_df["rhythm_class"].eq("AF/AFL")
            & export_df["beat_class"].eq("PAC")
        ):
            raise RuntimeError("PAC must be masked inside AF/AFL")

    output_csv.parent.mkdir(parents=True, exist_ok=True)
    export_df.to_csv(output_csv, index=False, encoding="utf-8-sig")

    print("saved:", output_csv)
    print("beats:", f"{len(export_df):,}")
    print("Unknown=1:", f"{int(export_df['Unknown'].sum()) if len(export_df) else 0:,}")
    print("short_run_flag=1:", f"{int(export_df['short_run_flag'].sum()) if len(export_df) else 0:,}")
    print("RUN candidates before Unknown:", f"{len(run_candidate_df):,}")
    print("FINAL RUNs after Unknown:", f"{len(final_run_df):,}")
    print("Unknown reference median:", f"{unknown_median:.8g}")
    print("Unknown threshold (median*0.20):", f"{unknown_threshold:.8g}")
    return export_df
