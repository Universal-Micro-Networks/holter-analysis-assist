"""BeatSense inference post-processing reference.

責務
----
- beat/Event/rhythm出力の時間統合
- overlap window由来beatの重複除去
- PAC/PVC/N判定とAF/AFL中PAC抑制に必要な補助処理
- RRI由来RUN候補検出（Direct Previous-3 Median）
- 既存仕様互換のUnknown判定
- Unknown重複RUNの最終除外

モデル入力の生成は ``preprocess.py``、モデル構築/weight loadは ``model.py`` が担当します。
"""

from __future__ import annotations

from bisect import bisect_right
from typing import Iterable, Sequence

import numpy as np
import pandas as pd
from scipy.signal import find_peaks, resample_poly

from .model import WINDOW_SAMPLES
from .preprocess import (
    ORIG_FS,
    FS,
    UPSAMPLE_FACTOR,
    WINDOW_CENTER,
    valid_range_250,
)


# =============================================================================
# Beat / EVENT / rhythm thresholds
# =============================================================================

BEAT_THRESHOLD = 0.90
PAC_THRESHOLD = 0.85
PVC_THRESHOLD = 0.85
AF_THRESHOLD = 0.85

MIN_PEAK_DISTANCE_MS = 120.0
MIN_PEAK_DISTANCE = int(round(FS * MIN_PEAK_DISTANCE_MS / 1000.0))
QRS_CLUSTER_TOL_MS = 80.0
QRS_CLUSTER_TOL = int(round(FS * QRS_CLUSTER_TOL_MS / 1000.0))
EVENT_VECTOR_SEARCH_MS = 40.0
EVENT_VECTOR_SEARCH = int(round(FS * EVENT_VECTOR_SEARCH_MS / 1000.0))

EVENT_PAC = 0
EVENT_PVC = 1
EVENT_N = 2


# =============================================================================
# RRI RUN specification
# =============================================================================

RUN_SHORT_MIN_BEATS = 4
RUN_LONG_MIN_BEATS = 30
RUN_SHORT_RRI_RATIO_4_TO_29 = 0.85
RUN_SHORT_RRI_RATIO_30_PLUS = 0.90
RUN_MIN_HR_BPM_4_TO_29 = 100.0
RUN_MIN_HR_BPM_30_PLUS = 90.0
RUN_REFERENCE_RRI_COUNT = 3
RUN_SUPPRESS_IN_AF = True


# =============================================================================
# Unknown specification
# =============================================================================

UNKNOWN_LOW_FREQ_HZ = 0.0
UNKNOWN_HIGH_FREQ_HZ = 40.0
UNKNOWN_AMPLITUDE_RATIO_THRESHOLD = 0.20
UNKNOWN_INTERVAL_MARGIN_SEC = 10.0
UNKNOWN_ABSOLUTE_AREA_EPS = 1e-12

UNKNOWN_LEGACY_WIN_SEC = 10.0
UNKNOWN_LEGACY_OVERLAP_SEC = 3.0
UNKNOWN_LEGACY_STEP_SEC = UNKNOWN_LEGACY_WIN_SEC - UNKNOWN_LEGACY_OVERLAP_SEC
UNKNOWN_LEGACY_SEG_LEN_250 = int(round(ORIG_FS * UNKNOWN_LEGACY_WIN_SEC))
UNKNOWN_LEGACY_STEP_LEN_250 = int(round(ORIG_FS * UNKNOWN_LEGACY_STEP_SEC))
UNKNOWN_LEGACY_SEG_LEN_500 = int(round(FS * UNKNOWN_LEGACY_WIN_SEC))

def same_time_event_vector(
    event_window: np.ndarray,
    local_position: int,
    search_radius: int,
):
    """beat位置±40 msでEVENT 3chの最大応答sampleをanchorとして採用する。"""
    lo = max(0, int(local_position) - int(search_radius))
    hi = min(len(event_window), int(local_position) + int(search_radius) + 1)
    if hi <= lo:
        return None

    local = np.asarray(event_window[lo:hi, :], dtype=np.float32)
    anchor = lo + int(np.argmax(np.max(local, axis=1)))
    vector = np.asarray(event_window[anchor, :], dtype=np.float32)
    if vector.shape != (3,) or not np.all(np.isfinite(vector)):
        return None
    return vector, anchor

def extract_window_candidates(
    beat_pred: np.ndarray,
    event_pred: np.ndarray,
    starts_abs_500: np.ndarray,
    global_window_offset: int,
) -> list[dict]:
    """各20秒windowからbeat候補と同時刻EVENT scoreを抽出する。"""
    candidates: list[dict] = []

    if beat_pred.ndim == 3:
        beat_pred = beat_pred[..., 0]

    for wi in range(len(starts_abs_500)):
        peaks, props = find_peaks(
            beat_pred[wi],
            height=BEAT_THRESHOLD,
            distance=MIN_PEAK_DISTANCE,
        )
        for local_pos, beat_score in zip(peaks, props["peak_heights"]):
            local_pos = int(local_pos)
            info = same_time_event_vector(
                event_pred[wi], local_pos, EVENT_VECTOR_SEARCH
            )
            if info is None:
                continue
            vec, anchor = info
            candidates.append({
                "abs_pos": int(starts_abs_500[wi]) + local_pos,
                "local_pos": local_pos,
                "window_index": int(global_window_offset + wi),
                "beat_score": float(beat_score),
                "center_distance": abs(float(local_pos) - WINDOW_CENTER),
                "pac_score": float(vec[EVENT_PAC]),
                "pvc_score": float(vec[EVENT_PVC]),
                "n_score": float(vec[EVENT_N]),
                "event_anchor_local": int(anchor),
            })
    return candidates

def cluster_candidates(candidates: Sequence[dict]) -> list[list[dict]]:
    """overlap window由来の同一QRS候補を±80 msでcluster化する。"""
    if not candidates:
        return []

    candidates = sorted(candidates, key=lambda x: x["abs_pos"])
    clusters: list[list[dict]] = []
    current = [candidates[0]]

    for c in candidates[1:]:
        center = float(np.median([x["abs_pos"] for x in current]))
        if abs(c["abs_pos"] - center) <= QRS_CLUSTER_TOL:
            current.append(c)
        else:
            clusters.append(current)
            current = [c]
    clusters.append(current)
    return clusters

def center_best_beats(clusters: Sequence[Sequence[dict]]) -> list[dict]:
    """QRS位置は最高beat score、EVENT scoreはwindow中央に最も近い候補を採用。"""
    beats: list[dict] = []
    for cluster in clusters:
        qrs_best = max(cluster, key=lambda x: x["beat_score"])
        class_best = min(
            cluster,
            key=lambda x: (x["center_distance"], -x["beat_score"]),
        )
        beats.append({
            "sample_in_file_500": int(qrs_best["abs_pos"]),
            "beat_score": float(qrs_best["beat_score"]),
            "pac_score": float(class_best["pac_score"]),
            "pvc_score": float(class_best["pvc_score"]),
            "n_score": float(class_best["n_score"]),
        })
    return sorted(beats, key=lambda x: x["sample_in_file_500"])

def classify_event_sigmoid(pac: float, pvc: float) -> str:
    """EVENT sigmoid scoreからPAC/PVC/Nを決定する。"""
    pac_pos = float(pac) >= PAC_THRESHOLD
    pvc_pos = float(pvc) >= PVC_THRESHOLD

    if pac_pos and pvc_pos:
        return "PAC" if float(pac) >= float(pvc) else "PVC"
    if pac_pos:
        return "PAC"
    if pvc_pos:
        return "PVC"
    return "N"

def center_ownership_intervals(window_rows: Sequence[dict]) -> list[tuple[int, int, int]]:
    """3秒overlapのrhythm windowを中心境界で一意なownership区間へ変換する。"""
    if not window_rows:
        return []

    starts = np.asarray([r["start_sample_500"] for r in window_rows], dtype=np.int64)
    ends = np.asarray([r["end_sample_500"] for r in window_rows], dtype=np.int64)
    centers = (starts.astype(float) + ends.astype(float) - 1.0) / 2.0

    out: list[tuple[int, int, int]] = []
    for i in range(len(window_rows)):
        ws, we = int(starts[i]), int(ends[i])
        os = ws if i == 0 else int(np.floor((centers[i - 1] + centers[i]) / 2.0) + 1)
        oe = we if i == len(window_rows) - 1 else int(np.floor((centers[i] + centers[i + 1]) / 2.0) + 1)
        os, oe = max(os, ws), min(oe, we)
        if oe > os:
            out.append((i, os, oe))
    return out

def merge_intervals(intervals: Iterable[tuple[int, int]]) -> list[tuple[int, int]]:
    """重複または接している[start,end) intervalをmergeする。"""
    intervals = sorted((int(a), int(b)) for a, b in intervals if int(b) > int(a))
    if not intervals:
        return []

    merged = [[intervals[0][0], intervals[0][1]]]
    for a, b in intervals[1:]:
        if a <= merged[-1][1]:
            merged[-1][1] = max(merged[-1][1], b)
        else:
            merged.append([a, b])
    return [(int(a), int(b)) for a, b in merged]

def build_rhythm_intervals(window_rows: list[dict]) -> tuple[list[dict], list[tuple[int, int]]]:
    """window rhythm scoreをownership区間に割当て、連続同一labelをmergeする。"""
    window_rows = sorted(
        window_rows,
        key=lambda x: (x["start_sample_500"], x["end_sample_500"]),
    )

    owned: list[dict] = []
    for i, own_start, own_end in center_ownership_intervals(window_rows):
        score = float(window_rows[i]["rhythm_score"])
        label = "AF/AFL" if score >= AF_THRESHOLD else "SR"
        owned.append({"start": own_start, "end": own_end, "label": label})

    # 隣接し同一labelなら1区間にまとめる。
    merged_rhythm: list[dict] = []
    for item in owned:
        if (
            merged_rhythm
            and merged_rhythm[-1]["label"] == item["label"]
            and merged_rhythm[-1]["end"] == item["start"]
        ):
            merged_rhythm[-1]["end"] = item["end"]
        else:
            merged_rhythm.append(dict(item))

    coverage = merge_intervals((x["start"], x["end"]) for x in owned)
    return merged_rhythm, coverage

def label_points_by_rhythm(
    points: np.ndarray,
    rhythm_intervals: Sequence[dict],
) -> np.ndarray:
    """beat sample位置ごとにAF/AFL / SR / UNCOVEREDを付与する。"""
    points = np.asarray(points, dtype=np.int64)
    labels = np.full(len(points), "UNCOVERED", dtype=object)
    if len(points) == 0 or not rhythm_intervals:
        return labels

    starts = [int(x["start"]) for x in rhythm_intervals]
    for j, p in enumerate(points):
        i = bisect_right(starts, int(p)) - 1
        if i >= 0:
            item = rhythm_intervals[i]
            if int(item["start"]) <= int(p) < int(item["end"]):
                labels[j] = item["label"]
    return labels

def point_in_intervals(points: np.ndarray, intervals: Sequence[tuple[int, int]]) -> np.ndarray:
    """各pointがいずれかの[start,end) interval内かを返す。"""
    points = np.asarray(points, dtype=np.int64)
    out = np.zeros(len(points), dtype=bool)
    for a, b in intervals:
        out |= (points >= int(a)) & (points < int(b))
    return out

def complement_intervals(
    start: int,
    end: int,
    covered: Sequence[tuple[int, int]],
) -> list[tuple[int, int]]:
    """[start,end)内のcoverage gapを返す。"""
    covered = merge_intervals(
        (max(start, a), min(end, b)) for a, b in covered if b > start and a < end
    )
    gaps: list[tuple[int, int]] = []
    cur = int(start)
    for a, b in covered:
        if a > cur:
            gaps.append((cur, a))
        cur = max(cur, b)
    if cur < end:
        gaps.append((cur, int(end)))
    return gaps

def make_interval_index(intervals: Sequence[tuple[int, int]]):
    """重複判定を高速化するためintervalをstart順に保持する。"""
    intervals = sorted((int(a), int(b)) for a, b in intervals if int(b) > int(a))
    return intervals, [a for a, _ in intervals]

def interval_overlaps(index, start_abs: int, end_abs: int) -> bool:
    """半開区間 [start_abs, end_abs) がindex中のintervalと重なるか。"""
    intervals, starts = index
    if not intervals or end_abs <= start_abs:
        return False

    i = bisect_right(starts, int(start_abs)) - 1
    if i >= 0 and intervals[i][1] > int(start_abs):
        return True
    j = i + 1
    return j < len(intervals) and intervals[j][0] < int(end_abs)

def detect_run_candidates(
    beat_df: pd.DataFrame,
    af_intervals: Sequence[tuple[int, int]],
    gap_intervals: Sequence[tuple[int, int]],
) -> tuple[pd.DataFrame, pd.DataFrame, pd.DataFrame, dict]:
    """RRI由来RUN候補をDirect Previous-3 Median方式で検出する。

    BeatSense reference specificationのRUN定義を使用する。

    Reference RR selection
    ----------------------
    rr[k] は beat[k] -> beat[k+1] のRRIとする。candidate onsetを rr[i]
    としたとき、referenceは以下で固定する。

        ref_idx = [i-3, i-2, i-1]
        reference_RRI = median(rr[ref_idx])

    重要:
      * 参照するのは「直前3本そのもの」であり、後方探索はしない。
      * 3本はすべてfinite/positiveかつAF/AFL・coverage gap外である必要がある。
      * 1本でも使用不可なら、そのcandidate onsetはスキップする。
      * 24時間Q75、global reference pool、2-pass cleaningは使用しない。
      * onsetで求めたmedianはcandidate全体で固定し、RUN途中のRRIを新しい
        referenceとして使用しない。

    RUN threshold
    -------------
    * 4-29 beats:
        every member RR <= 0.85 * frozen_reference_RRI
        and 60 / mean(member RR) >= 100 bpm
    * >=30 beats:
        every member RR <= 0.90 * frozen_reference_RRI
        and 60 / mean(member RR) >= 90 bpm

    最終長はonset時には不明なため、同じfrozen referenceを用いてまず0.90
    thresholdの最大連続列を評価する。>=30 beatsかつHR条件を満たせばlong RUN
    として採用し、それ以外では0.85 thresholdの4-29 beatsを評価する。

    Returns
    -------
    candidate_df:
        Unknown適用前のRUN候補（1行=1 RUN）。
    short_df:
        RUN候補に含まれる短縮RRI endpoint beat（1行=1 member beat）。
    rr_audit:
        各RRIについて、AF/gap overlap、usable、直前3RRIとmedianを記録した監査表。
    reference_summary:
        Direct Previous-3 Median方式の設定値と候補件数の要約。
    """
    required = {"sample_in_file_500", "timestamp", "class", "pac_score", "pvc_score"}
    missing = sorted(required.difference(beat_df.columns))
    if missing:
        raise ValueError(f"beat_df is missing required columns: {missing}")

    beat_df = beat_df.sort_values("sample_in_file_500", kind="mergesort").reset_index(drop=True)
    pos = beat_df["sample_in_file_500"].to_numpy(dtype=np.int64)

    short_columns = [
        "candidate_run_id", "timestamp", "sample_in_file_500", "beat_index", "rr_index",
        "rri_start_sample_in_file_500", "rri_end_sample_in_file_500",
        "rri_start_time", "rri_end_time", "rri_sec", "rri_pass_fixed_threshold",
        "run_length_class", "reference_rr_count", "reference_method",
        "reference_median_previous_rr_sec", "reference_rr_values_sec",
        "short_rri_ratio_threshold", "short_rri_threshold_sec",
        "run_heart_rate_bpm", "min_heart_rate_bpm_required",
        "rri_ratio_to_reference", "rri_ratio_to_previous",
        "run_member_number", "model_class", "pac_score", "pvc_score",
    ]
    run_columns = [
        "candidate_run_id", "start_time", "end_time", "duration_sec",
        "start_sample_in_file_500", "end_sample_in_file_500",
        "first_short_rri_start_sample_in_file_500", "last_short_rri_end_sample_in_file_500",
        "n_short_beats", "run_length_class", "reference_rr_count", "reference_method",
        "reference_median_previous_rr_sec", "reference_rr_values_sec",
        "short_rri_ratio_threshold", "short_rri_threshold_sec",
        "min_rr_sec", "mean_rr_sec", "max_rr_sec", "run_heart_rate_bpm",
        "min_heart_rate_bpm_required", "heart_rate_definition",
        "PAC_labeled_beats", "PVC_labeled_beats", "N_labeled_beats",
        "continuation_rule",
    ]
    audit_columns = [
        "rr_index", "start_beat_index", "end_beat_index", "start_time", "end_time", "rri_sec",
        "overlaps_gap", "overlaps_af", "usable_non_af_non_gap",
        "direct_previous3_available",
        "direct_previous3_rr1_sec", "direct_previous3_rr2_sec", "direct_previous3_rr3_sec",
        "direct_previous3_median_rr_sec", "final_run_candidate_member",
    ]

    if len(pos) < 2:
        return (
            pd.DataFrame(columns=run_columns),
            pd.DataFrame(columns=short_columns),
            pd.DataFrame(columns=audit_columns),
            {
                "run_detection_skipped": True,
                "reason": "fewer_than_2_detected_beats",
                "reference_method": "direct_previous3_median",
            },
        )
    if np.any(np.diff(pos) <= 0):
        raise RuntimeError("Beat positions must be strictly increasing")

    if int(RUN_REFERENCE_RRI_COUNT) != 3:
        raise ValueError(
            "DIRECT PREVIOUS-3 MEDIAN requires RUN_REFERENCE_RRI_COUNT == 3; "
            f"got {RUN_REFERENCE_RRI_COUNT}"
        )
    reference_n = 3

    if int(RUN_SHORT_MIN_BEATS) != 4:
        raise ValueError("RUN_SHORT_MIN_BEATS is expected to be 4")
    if int(RUN_LONG_MIN_BEATS) <= int(RUN_SHORT_MIN_BEATS):
        raise ValueError("RUN_LONG_MIN_BEATS must be greater than RUN_SHORT_MIN_BEATS")
    if not (
        0.0
        < float(RUN_SHORT_RRI_RATIO_4_TO_29)
        < float(RUN_SHORT_RRI_RATIO_30_PLUS)
        < 1.0
    ):
        raise ValueError("Expected 0 < short ratio(4-29) < short ratio(30+) < 1")
    if float(RUN_MIN_HR_BPM_4_TO_29) <= 0 or float(RUN_MIN_HR_BPM_30_PLUS) <= 0:
        raise ValueError("RUN minimum heart-rate thresholds must be > 0")
    if float(RUN_MIN_HR_BPM_4_TO_29) < float(RUN_MIN_HR_BPM_30_PLUS):
        raise ValueError(
            "Expected RUN_MIN_HR_BPM_4_TO_29 >= RUN_MIN_HR_BPM_30_PLUS"
        )

    # rr[k] = beat[k] -> beat[k+1]
    rr = np.diff(pos).astype(np.float64) / FS
    n_rr = len(rr)
    af_index = make_interval_index(af_intervals)
    gap_index = make_interval_index(gap_intervals)

    # AF/AFL・coverage gapに重なるRRIは、candidate memberにもreferenceにも使わない。
    rr_usable = np.zeros(n_rr, dtype=bool)
    rr_overlaps_gap = np.zeros(n_rr, dtype=bool)
    rr_overlaps_af = np.zeros(n_rr, dtype=bool)

    for k in range(n_rr):
        a = int(pos[k])
        b = int(pos[k + 1]) + 1
        in_gap = interval_overlaps(gap_index, a, b)
        in_af = RUN_SUPPRESS_IN_AF and interval_overlaps(af_index, a, b)
        rr_overlaps_gap[k] = bool(in_gap)
        rr_overlaps_af[k] = bool(in_af)
        if in_gap or in_af:
            continue
        value = float(rr[k])
        if np.isfinite(value) and value > 0.0:
            rr_usable[k] = True

    def direct_previous3_reference(before_rr_index: int):
        """rr[i]の直前3本 rr[i-3:i] とそのmedianを返す。

        backward skippingはしない。直前3本のうち1本でも使用不可ならNone。
        """
        i = int(before_rr_index)
        if i < reference_n:
            return None

        ref_idx = np.arange(i - reference_n, i, dtype=np.int64)
        if len(ref_idx) != reference_n or not np.all(rr_usable[ref_idx]):
            return None

        ref_values = np.asarray(rr[ref_idx], dtype=np.float64)
        if not np.all(np.isfinite(ref_values) & (ref_values > 0.0)):
            return None

        reference_median = float(np.median(ref_values))
        if not np.isfinite(reference_median) or reference_median <= 0.0:
            return None

        return ref_idx, ref_values, reference_median

    def grow_same_threshold_sequence(start_rr_index: int, threshold: float):
        """同じfrozen thresholdを満たす最大連続RRI列を伸ばす。"""
        members: list[int] = []
        values: list[float] = []
        j = int(start_rr_index)
        while j < n_rr:
            if not rr_usable[j]:
                break
            cur = float(rr[j])
            if not np.isfinite(cur) or cur <= 0.0 or cur > float(threshold):
                break
            members.append(int(j))
            values.append(cur)
            j += 1
        return members, values, j

    def detect_with_direct_previous3_median():
        events: list[dict] = []
        i_rr = reference_n  # それ以前は直前3RRIを持てない

        while i_rr < n_rr:
            if not rr_usable[i_rr]:
                i_rr += 1
                continue

            ref_info = direct_previous3_reference(i_rr)
            if ref_info is None:
                i_rr += 1
                continue

            ref_idx, ref_values, reference_median = ref_info
            strict_threshold = float(RUN_SHORT_RRI_RATIO_4_TO_29) * reference_median
            relaxed_threshold = float(RUN_SHORT_RRI_RATIO_30_PLUS) * reference_median

            # まず同一referenceで>=30 beats候補を0.90 thresholdで評価する。
            relaxed_members, relaxed_values, relaxed_end = grow_same_threshold_sequence(
                i_rr, relaxed_threshold
            )
            if len(relaxed_members) >= int(RUN_LONG_MIN_BEATS):
                relaxed_mean_rr = float(np.mean(relaxed_values))
                relaxed_hr_bpm = (
                    60.0 / relaxed_mean_rr
                    if np.isfinite(relaxed_mean_rr) and relaxed_mean_rr > 0.0
                    else np.nan
                )
                if (
                    np.isfinite(relaxed_hr_bpm)
                    and relaxed_hr_bpm >= float(RUN_MIN_HR_BPM_30_PLUS)
                ):
                    events.append({
                        "member_rr_indices": np.asarray(relaxed_members, dtype=np.int64),
                        "reference_rr_indices": np.asarray(ref_idx, dtype=np.int64),
                        "reference_rr_values": np.asarray(ref_values, dtype=np.float64),
                        "reference_median": reference_median,
                        "short_ratio": float(RUN_SHORT_RRI_RATIO_30_PLUS),
                        "short_threshold": relaxed_threshold,
                        "member_rr_values": np.asarray(relaxed_values, dtype=np.float64),
                        "run_length_class": "30_plus",
                        "run_heart_rate_bpm": float(relaxed_hr_bpm),
                        "min_heart_rate_bpm_required": float(RUN_MIN_HR_BPM_30_PLUS),
                    })
                    i_rr = relaxed_end
                    continue
                # >=30本伸びてもHRが不足した場合、同じonsetからstrict列も評価する。

            strict_members, strict_values, strict_end = grow_same_threshold_sequence(
                i_rr, strict_threshold
            )
            if int(RUN_SHORT_MIN_BEATS) <= len(strict_members) < int(RUN_LONG_MIN_BEATS):
                strict_mean_rr = float(np.mean(strict_values))
                strict_hr_bpm = (
                    60.0 / strict_mean_rr
                    if np.isfinite(strict_mean_rr) and strict_mean_rr > 0.0
                    else np.nan
                )
                if (
                    np.isfinite(strict_hr_bpm)
                    and strict_hr_bpm >= float(RUN_MIN_HR_BPM_4_TO_29)
                ):
                    events.append({
                        "member_rr_indices": np.asarray(strict_members, dtype=np.int64),
                        "reference_rr_indices": np.asarray(ref_idx, dtype=np.int64),
                        "reference_rr_values": np.asarray(ref_values, dtype=np.float64),
                        "reference_median": reference_median,
                        "short_ratio": float(RUN_SHORT_RRI_RATIO_4_TO_29),
                        "short_threshold": strict_threshold,
                        "member_rr_values": np.asarray(strict_values, dtype=np.float64),
                        "run_length_class": "4_to_29",
                        "run_heart_rate_bpm": float(strict_hr_bpm),
                        "min_heart_rate_bpm_required": float(RUN_MIN_HR_BPM_4_TO_29),
                    })
                    i_rr = strict_end
                    continue

            i_rr += 1

        return events

    final_events = detect_with_direct_previous3_median()

    short_rows: list[dict] = []
    run_rows: list[dict] = []
    run_member_rr_mask = np.zeros(n_rr, dtype=bool)

    for candidate_run_id, ev in enumerate(final_events, start=1):
        member_rr_idx = np.asarray(ev["member_rr_indices"], dtype=np.int64)
        member_beat_idx = member_rr_idx + 1  # rr[k]のendpoint beatがRUN member beat
        start_beat_idx = int(member_beat_idx[0])
        end_beat_idx = int(member_beat_idx[-1])
        start_abs = int(pos[start_beat_idx])
        end_abs = int(pos[end_beat_idx])
        first_rr_start_abs = int(pos[int(member_rr_idx[0])])
        last_rr_end_abs = int(pos[int(member_rr_idx[-1]) + 1])

        run_member_rr_mask[member_rr_idx] = True

        classes = beat_df.iloc[member_beat_idx]["class"].astype(str)
        ref_idx = np.asarray(ev["reference_rr_indices"], dtype=np.int64)
        ref_values = np.asarray(ev["reference_rr_values"], dtype=np.float64)
        reference_median = float(ev["reference_median"])
        member_rrs = np.asarray(ev["member_rr_values"], dtype=np.float64)
        threshold = float(ev["short_threshold"])
        ratio_threshold = float(ev["short_ratio"])
        length_class = str(ev["run_length_class"])
        run_hr = float(ev["run_heart_rate_bpm"])
        min_hr = float(ev["min_heart_rate_bpm_required"])

        if len(member_rr_idx) == 0:
            raise RuntimeError(f"Empty RUN candidate #{candidate_run_id}")
        if len(member_rr_idx) > 1 and not np.all(np.diff(member_rr_idx) == 1):
            raise RuntimeError(f"Non-consecutive RR members in RUN #{candidate_run_id}")
        if len(member_rrs) != len(member_rr_idx):
            raise RuntimeError(f"RUN member RR length mismatch in RUN #{candidate_run_id}")
        if not np.all(
            np.isfinite(member_rrs)
            & (member_rrs > 0.0)
            & (member_rrs <= threshold + 1e-12)
        ):
            raise RuntimeError(f"RUN #{candidate_run_id} violates frozen threshold")

        expected_ref_idx = np.arange(int(member_rr_idx[0]) - 3, int(member_rr_idx[0]), dtype=np.int64)
        if not np.array_equal(ref_idx, expected_ref_idx):
            raise RuntimeError(
                f"RUN #{candidate_run_id} direct-previous3 invariant failed: "
                f"got={ref_idx.tolist()}, expected={expected_ref_idx.tolist()}"
            )
        if len(ref_values) != 3 or not np.isclose(reference_median, np.median(ref_values)):
            raise RuntimeError(f"RUN #{candidate_run_id} median-reference invariant failed")

        run_rows.append({
            "candidate_run_id": int(candidate_run_id),
            "start_time": beat_df.iloc[start_beat_idx]["timestamp"],
            "end_time": beat_df.iloc[end_beat_idx]["timestamp"],
            "duration_sec": (end_abs - start_abs) / FS,
            "start_sample_in_file_500": start_abs,
            "end_sample_in_file_500": end_abs,
            "first_short_rri_start_sample_in_file_500": first_rr_start_abs,
            "last_short_rri_end_sample_in_file_500": last_rr_end_abs,
            "n_short_beats": int(len(member_beat_idx)),
            "run_length_class": length_class,
            "reference_rr_count": int(reference_n),
            "reference_method": "direct_previous3_median",
            "reference_median_previous_rr_sec": reference_median,
            "reference_rr_values_sec": ",".join(f"{x:.6f}" for x in ref_values),
            "short_rri_ratio_threshold": ratio_threshold,
            "short_rri_threshold_sec": threshold,
            "min_rr_sec": float(np.min(member_rrs)),
            "mean_rr_sec": float(np.mean(member_rrs)),
            "max_rr_sec": float(np.max(member_rrs)),
            "run_heart_rate_bpm": run_hr,
            "min_heart_rate_bpm_required": min_hr,
            "heart_rate_definition": "60 / mean(member_RRI_sec)",
            "PAC_labeled_beats": int(np.sum(classes == "PAC")),
            "PVC_labeled_beats": int(np.sum(classes == "PVC")),
            "N_labeled_beats": int(np.sum(classes == "N")),
            "continuation_rule": "same_fixed_direct_previous3_median_reference_for_all_members",
        })

        for local_member_no, (rr_idx, beat_idx) in enumerate(
            zip(member_rr_idx, member_beat_idx), start=1
        ):
            rr_idx = int(rr_idx)
            beat_idx = int(beat_idx)
            cur_rr = float(rr[rr_idx])
            prev_rr = float(rr[rr_idx - 1]) if rr_idx >= 1 else np.nan
            short_rows.append({
                "candidate_run_id": int(candidate_run_id),
                "timestamp": beat_df.iloc[beat_idx]["timestamp"],
                "sample_in_file_500": int(pos[beat_idx]),
                "beat_index": beat_idx,
                "rr_index": rr_idx,
                "rri_start_sample_in_file_500": int(pos[rr_idx]),
                "rri_end_sample_in_file_500": int(pos[rr_idx + 1]),
                "rri_start_time": beat_df.iloc[rr_idx]["timestamp"],
                "rri_end_time": beat_df.iloc[rr_idx + 1]["timestamp"],
                "rri_sec": cur_rr,
                "rri_pass_fixed_threshold": 1,
                "run_length_class": length_class,
                "reference_rr_count": int(reference_n),
                "reference_method": "direct_previous3_median",
                "reference_median_previous_rr_sec": reference_median,
                "reference_rr_values_sec": ",".join(f"{x:.6f}" for x in ref_values),
                "short_rri_ratio_threshold": ratio_threshold,
                "short_rri_threshold_sec": threshold,
                "run_heart_rate_bpm": run_hr,
                "min_heart_rate_bpm_required": min_hr,
                "rri_ratio_to_reference": (
                    cur_rr / reference_median if reference_median > 0.0 else np.nan
                ),
                # 監査用。RUN継続判定には使用しない。
                "rri_ratio_to_previous": (
                    cur_rr / prev_rr if np.isfinite(prev_rr) and prev_rr > 0.0 else np.nan
                ),
                "run_member_number": int(local_member_no),
                "model_class": str(beat_df.iloc[beat_idx]["class"]),
                "pac_score": float(beat_df.iloc[beat_idx]["pac_score"]),
                "pvc_score": float(beat_df.iloc[beat_idx]["pvc_score"]),
            })

    candidate_df = pd.DataFrame(run_rows, columns=run_columns)
    short_df = pd.DataFrame(short_rows, columns=short_columns)

    direct3_available = np.zeros(n_rr, dtype=np.int8)
    direct3_rr1 = np.full(n_rr, np.nan, dtype=np.float64)
    direct3_rr2 = np.full(n_rr, np.nan, dtype=np.float64)
    direct3_rr3 = np.full(n_rr, np.nan, dtype=np.float64)
    direct3_median = np.full(n_rr, np.nan, dtype=np.float64)

    for i in range(reference_n, n_rr):
        info = direct_previous3_reference(i)
        if info is None:
            continue
        _, vals, med = info
        direct3_available[i] = 1
        direct3_rr1[i], direct3_rr2[i], direct3_rr3[i] = map(float, vals)
        direct3_median[i] = float(med)

    rr_audit = pd.DataFrame({
        "rr_index": np.arange(n_rr, dtype=np.int64),
        "start_beat_index": np.arange(n_rr, dtype=np.int64),
        "end_beat_index": np.arange(1, n_rr + 1, dtype=np.int64),
        "start_time": beat_df.iloc[:-1]["timestamp"].to_numpy(),
        "end_time": beat_df.iloc[1:]["timestamp"].to_numpy(),
        "rri_sec": rr,
        "overlaps_gap": rr_overlaps_gap.astype(np.int8),
        "overlaps_af": rr_overlaps_af.astype(np.int8),
        "usable_non_af_non_gap": rr_usable.astype(np.int8),
        "direct_previous3_available": direct3_available,
        "direct_previous3_rr1_sec": direct3_rr1,
        "direct_previous3_rr2_sec": direct3_rr2,
        "direct_previous3_rr3_sec": direct3_rr3,
        "direct_previous3_median_rr_sec": direct3_median,
        "final_run_candidate_member": run_member_rr_mask.astype(np.int8),
    }, columns=audit_columns)

    reference_summary = {
        "run_detection_skipped": False,
        "reference_method": "direct_previous3_median",
        "reference_rr_count_per_onset": int(reference_n),
        "reference_definition": (
            "median of exactly the three immediately preceding consecutive usable RRIs; "
            "all 3 must be finite/positive and outside AF/AFL and coverage gaps; "
            "no backward skipping; no 24h Q75/global reference pool"
        ),
        "usable_non_af_non_gap_rr_count": int(np.sum(rr_usable)),
        "rr_overlapping_af_count": int(np.sum(rr_overlaps_af)),
        "rr_overlapping_gap_count": int(np.sum(rr_overlaps_gap)),
        "direct_previous3_eligible_onset_count": int(np.sum(direct3_available)),
        "run_short_rri_ratio_4_to_29": float(RUN_SHORT_RRI_RATIO_4_TO_29),
        "run_short_rri_ratio_30_plus": float(RUN_SHORT_RRI_RATIO_30_PLUS),
        "run_min_hr_bpm_4_to_29": float(RUN_MIN_HR_BPM_4_TO_29),
        "run_min_hr_bpm_30_plus": float(RUN_MIN_HR_BPM_30_PLUS),
        "run_heart_rate_definition": "60 / mean(member_RRI_sec)",
        "continuation_definition": (
            "all RUN members use the same frozen onset reference_RRI; "
            "previous RUN-member RR is audit-only and never becomes the reference"
        ),
        "run_short_min_beats": int(RUN_SHORT_MIN_BEATS),
        "run_long_min_beats": int(RUN_LONG_MIN_BEATS),
        "final_run_candidate_count": int(len(candidate_df)),
        "final_run_4_to_29_count": (
            int(np.sum(candidate_df["run_length_class"].eq("4_to_29")))
            if len(candidate_df) else 0
        ),
        "final_run_30_plus_count": (
            int(np.sum(candidate_df["run_length_class"].eq("30_plus")))
            if len(candidate_df) else 0
        ),
        "run_suppress_in_af": bool(RUN_SUPPRESS_IN_AF),
    }
    return candidate_df, short_df, rr_audit, reference_summary

def finalize_runs_after_unknown(
    candidate_df: pd.DataFrame,
    candidate_short_df: pd.DataFrame,
    unknown_intervals: Sequence[tuple[int, int]],
) -> tuple[pd.DataFrame, pd.DataFrame, pd.DataFrame, set[int]]:
    """FINAL Unknown適用後のRUNとshort_run_flag対象beatを確定する。

    RUN候補がFINAL Unknown intervalに1 sampleでも重なれば、そのRUN候補全体を
    除外する。受け入れられたRUNのmember RRI endpoint beatだけが、最終CSVで
    short_run_flag=1となる。
    """
    final_run_columns = [
        "run_id", "candidate_run_id", "start_time", "end_time", "duration_sec",
        "start_sample_in_file_500", "end_sample_in_file_500", "n_short_beats",
        "run_length_class", "reference_rr_count", "reference_method",
        "reference_median_previous_rr_sec", "reference_rr_values_sec",
        "short_rri_ratio_threshold", "short_rri_threshold_sec",
        "min_rr_sec", "mean_rr_sec", "max_rr_sec", "run_heart_rate_bpm",
        "min_heart_rate_bpm_required", "heart_rate_definition",
        "PAC_labeled_beats", "PVC_labeled_beats", "N_labeled_beats",
        "continuation_rule", "Unknown", "final_label",
    ]

    rejected = pd.DataFrame()
    if len(candidate_df):
        if "reference_method" not in candidate_df.columns:
            raise RuntimeError(
                "RUN candidates have no reference_method column; "
                "rebuild candidates with Direct Previous-3 Median."
            )
        methods = set(candidate_df["reference_method"].astype(str).dropna().unique().tolist())
        if methods != {"direct_previous3_median"}:
            raise RuntimeError(
                f"RUN candidate reference_method mismatch: {sorted(methods)}; "
                "expected direct_previous3_median"
            )

        unknown_index = make_interval_index(unknown_intervals)
        overlaps_unknown = np.asarray([
            interval_overlaps(
                unknown_index,
                int(r.start_sample_in_file_500),
                int(r.end_sample_in_file_500) + 1,
            )
            for r in candidate_df.itertuples(index=False)
        ], dtype=bool)

        rejected = candidate_df.loc[overlaps_unknown].copy()
        rejected["rejection_reason"] = "overlaps_frequency_unknown"

        final_runs = candidate_df.loc[~overlaps_unknown].copy().reset_index(drop=True)
        final_runs.insert(0, "run_id", np.arange(1, len(final_runs) + 1, dtype=np.int64))
        final_runs["Unknown"] = 0
        final_runs["final_label"] = "RUN"
    else:
        final_runs = pd.DataFrame(columns=final_run_columns)

    for col in final_run_columns:
        if col not in final_runs.columns:
            final_runs[col] = pd.Series(dtype="object")
    final_runs = final_runs[final_run_columns]

    if len(candidate_short_df) and len(final_runs):
        id_map = dict(zip(
            final_runs["candidate_run_id"].astype(int),
            final_runs["run_id"].astype(int),
        ))
        final_short = candidate_short_df[
            candidate_short_df["candidate_run_id"].astype(int).isin(id_map)
        ].copy()
        final_short["run_id"] = (
            final_short["candidate_run_id"].astype(int).map(id_map).astype(int)
        )
        final_short["Unknown"] = 0
    else:
        final_short = candidate_short_df.iloc[0:0].copy()
        final_short["run_id"] = pd.Series(dtype=np.int64)
        final_short["Unknown"] = pd.Series(dtype=np.int8)

    member_samples = set(
        pd.to_numeric(
            final_short.get("sample_in_file_500", pd.Series(dtype=float)),
            errors="coerce",
        )
        .dropna()
        .astype(np.int64)
        .tolist()
    )
    return final_runs, final_short, rejected, member_samples

def calc_amplitude_spectrum(x: np.ndarray, fs: float):
    """Hanning + coherent-gain補正を用いたone-sided amplitude spectrum。"""
    x = np.asarray(x, dtype=np.float64).reshape(-1)
    n = len(x)
    if n < 2:
        return np.array([], dtype=float), np.array([], dtype=float)

    x = np.nan_to_num(x, nan=0.0, posinf=0.0, neginf=0.0)
    x = x - np.mean(x)

    window = np.hanning(n)
    coherent_gain = float(np.mean(window))
    if not np.isfinite(coherent_gain) or coherent_gain <= 0:
        coherent_gain = 1.0

    fft_values = np.fft.rfft(x * window)
    frequencies = np.fft.rfftfreq(n, d=1.0 / float(fs))
    amplitude = np.abs(fft_values) / (n * coherent_gain)

    if n % 2 == 0:
        if len(amplitude) > 2:
            amplitude[1:-1] *= 2.0
    elif len(amplitude) > 1:
        amplitude[1:] *= 2.0

    return frequencies, amplitude

def calc_amplitude_area_0_40(x: np.ndarray, fs: float) -> float:
    """0 < f <= 40 Hzのamplitude spectrum面積。"""
    f, amp = calc_amplitude_spectrum(x, fs)
    if len(f) == 0:
        return np.nan

    mask = (f > UNKNOWN_LOW_FREQ_HZ) & (f <= UNKNOWN_HIGH_FREQ_HZ)
    if not np.any(mask):
        return np.nan

    ff = f[mask]
    aa = np.nan_to_num(amp[mask], nan=0.0, posinf=0.0, neginf=0.0)
    if hasattr(np, "trapezoid"):
        return float(np.trapezoid(aa, ff))
    return float(np.trapz(aa, ff))

def legacy_resample_windows_250_to_500(x_250: np.ndarray) -> np.ndarray:
    """Unknown QCでのみ使う10秒window単位の250→500 Hz resample。"""
    x_250 = np.asarray(x_250, dtype=np.float32)
    x_500 = resample_poly(
        x_250,
        up=FS,
        down=ORIG_FS,
        axis=1,
    ).astype(np.float32)

    if x_500.shape[1] > UNKNOWN_LEGACY_SEG_LEN_500:
        x_500 = x_500[:, :UNKNOWN_LEGACY_SEG_LEN_500]
    elif x_500.shape[1] < UNKNOWN_LEGACY_SEG_LEN_500:
        pad_len = UNKNOWN_LEGACY_SEG_LEN_500 - x_500.shape[1]
        x_500 = np.pad(x_500, ((0, 0), (0, pad_len)), mode="edge")
    return x_500.astype(np.float32)

def legacy_zscore_and_restore(x_500: np.ndarray, eps: float = 1e-6) -> np.ndarray:
    """旧NPZ処理のfloat32 z-score→inverseを再現し、FFT入力波形を作る。"""
    x_500 = np.asarray(x_500, dtype=np.float32)
    mean = x_500.mean(axis=1, keepdims=True)
    std = x_500.std(axis=1, keepdims=True)
    std = np.where(std < eps, 1.0, std)

    x_z = ((x_500 - mean) / std).astype(np.float32)
    mean32 = mean[:, 0].astype(np.float32)
    std32 = std[:, 0].astype(np.float32)

    return (
        x_z.astype(np.float64) * std32.astype(np.float64)[:, None]
        + mean32.astype(np.float64)[:, None]
    )

def build_unknown_intervals(
    ecg_all_250: np.ndarray,
    source_info: dict,
    timeline_start_abs_500: int,
    timeline_end_abs_500: int,
) -> tuple[list[tuple[int, int]], float, float]:
    """元ECLからFINAL Unknown intervalを生成する。

    BeatSense reference specificationと同じ手順:
      - 各clock-hourを独立に切る
      - hour内で10秒window / 3秒overlap / 7秒stepを毎時再スタート
      - 各windowを独立に250→500 Hz resample
      - 旧float32 z-score/inverse経路を通したpre-z-score波形でFFT
      - 0 < f <= 40 Hz amplitude面積
      - 当該ECL 1日中央値の20%未満（または非finite）をcore Unknown
      - coreをmerge後、±10秒拡張してFINAL Unknown
    """
    valid_start_250, valid_end_250 = valid_range_250(source_info, len(ecg_all_250))
    file_start_time = source_info["study_date"]
    recording_start = source_info["recording_start"]
    from .preprocess import recording_end_exclusive_capped
    recording_end_exclusive = recording_end_exclusive_capped(source_info)

    rows: list[tuple[int, int, float]] = []

    first_hour_index = max(0, int((recording_start - file_start_time).total_seconds() // 3600))
    last_hour_index_excl = int(
        ((recording_end_exclusive - file_start_time).total_seconds() + 3599) // 3600
    )

    for hour_index in range(first_hour_index, last_hour_index_excl):
        hour_start = file_start_time + pd.Timedelta(hours=hour_index)
        hour_end = hour_start + pd.Timedelta(hours=1)

        data_start_time = max(hour_start, recording_start)
        data_end_time = min(hour_end, recording_end_exclusive)
        if data_end_time <= data_start_time:
            continue

        sample_start = int(round(
            (data_start_time - file_start_time).total_seconds() * ORIG_FS
        ))
        sample_end = int(np.ceil(
            (data_end_time - file_start_time).total_seconds() * ORIG_FS
        ))
        sample_start = max(valid_start_250, sample_start)
        sample_end = min(valid_end_250, sample_end)

        if sample_end - sample_start < UNKNOWN_LEGACY_SEG_LEN_250:
            continue

        ecg_hour_250 = np.asarray(
            ecg_all_250[sample_start:sample_end], dtype=np.float32
        )
        starts_local = np.arange(
            0,
            len(ecg_hour_250) - UNKNOWN_LEGACY_SEG_LEN_250 + 1,
            UNKNOWN_LEGACY_STEP_LEN_250,
            dtype=np.int64,
        )
        if len(starts_local) == 0:
            continue

        x_250 = np.stack([
            ecg_hour_250[int(s):int(s) + UNKNOWN_LEGACY_SEG_LEN_250]
            for s in starts_local
        ], axis=0).astype(np.float32)

        x_500 = legacy_resample_windows_250_to_500(x_250)
        x_before_zscore = legacy_zscore_and_restore(x_500)

        abs_starts_250 = sample_start + starts_local
        abs_starts_500 = abs_starts_250 * UPSAMPLE_FACTOR
        abs_ends_500 = abs_starts_500 + UNKNOWN_LEGACY_SEG_LEN_500

        for i in range(len(x_before_zscore)):
            area = calc_amplitude_area_0_40(x_before_zscore[i], FS)
            rows.append((
                int(abs_starts_500[i]),
                int(abs_ends_500[i]),
                float(area) if np.isfinite(area) else np.nan,
            ))

    if not rows:
        raise RuntimeError("No Unknown-QC windows could be built")

    finite_areas = np.asarray([r[2] for r in rows], dtype=np.float64)
    finite_areas = finite_areas[np.isfinite(finite_areas)]
    if len(finite_areas) == 0:
        raise RuntimeError("No finite Unknown-QC amplitude values")

    median_area = float(np.median(finite_areas))
    if not np.isfinite(median_area) or median_area <= 0:
        raise RuntimeError(f"Invalid Unknown reference median: {median_area}")

    threshold = max(
        median_area * UNKNOWN_AMPLITUDE_RATIO_THRESHOLD,
        UNKNOWN_ABSOLUTE_AREA_EPS,
    )

    core_intervals = merge_intervals(
        (start, end)
        for start, end, area in rows
        if (not np.isfinite(area)) or area < threshold
    )

    margin = int(round(UNKNOWN_INTERVAL_MARGIN_SEC * FS))
    expanded = []
    for a, b in core_intervals:
        aa = max(int(timeline_start_abs_500), int(a) - margin)
        bb = min(int(timeline_end_abs_500), int(b) + margin)
        if bb > aa:
            expanded.append((aa, bb))

    return merge_intervals(expanded), median_area, threshold
