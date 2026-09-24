"""BeatSense ECL preprocessing reference.

責務
----
- ECLファイル名の解析
- ECL 16-bit wordから12-bit ECG ADC countを復号
- 有効記録区間の抽出
- AI入力用0.3-100 Hz zero-phase band-pass
- 250 Hz -> 500 Hz resampling
- 20秒window / 3秒overlap / window z-score

このモジュールは「モデルへ入力するtensorを作るところまで」を担当します。
Beat/PAC/PVC/AF/AFL、Unknown、RUNの判定は ``postprocess.py`` 側です。
"""

from __future__ import annotations

from pathlib import Path
import re

import numpy as np
import pandas as pd
from scipy.signal import butter, resample_poly, sosfiltfilt

from .model import MODEL_FS_HZ, WINDOW_SAMPLES


# =============================================================================
# ECL / model input specification
# =============================================================================

ORIG_FS = 250
FS = MODEL_FS_HZ
UPSAMPLE_FACTOR = FS // ORIG_FS
EXPECTED_24H_SAMPLES_250 = ORIG_FS * 24 * 60 * 60

WINDOW_SEC = 20.0
OVERLAP_SEC = 3.0
STEP_SEC = WINDOW_SEC - OVERLAP_SEC
STEP_SAMPLES = int(round(FS * STEP_SEC))
WINDOW_CENTER = (WINDOW_SAMPLES - 1) / 2.0

BPF_LOW_HZ = 0.3
BPF_HIGH_HZ = 100.0
BPF_ORDER = 4

ECL_DTYPE = np.dtype("<u2")
ECL_ZERO_LEVEL = np.int32(0x0800)
ECL_ECG_HIGH4_MASK = np.uint16(0xF000)
ECL_EVENT_MASK = np.uint16(0x0F00)
ECL_ECG_LOW8_MASK = np.uint16(0x00FF)

ECL_FILENAME_PATTERN = re.compile(
    r"^(?P<serial>\d{10})_(?P<study_date>\d{8})_"
    r"(?P<start_time>\d{4})_(?P<end_time>\d{4})\.ecl$",
    flags=re.IGNORECASE,
)

def parse_ecl_filename(ecl_path: str | Path) -> dict:
    """ECLファイル名から測定日と有効記録区間を得る。

    想定形式:
        [10桁serial]_[yyyyMMdd]_[HHmm]_[HHmm].ecl
    """
    ecl_path = Path(ecl_path)
    m = ECL_FILENAME_PATTERN.match(ecl_path.name)
    if m is None:
        raise ValueError(
            "Unexpected ECL filename; expected "
            "'[10-digit serial]_[yyyyMMdd]_[HHmm]_[HHmm].ecl': "
            f"{ecl_path.name}"
        )

    info = m.groupdict()
    study_date = pd.to_datetime(info["study_date"], format="%Y%m%d").normalize()
    start_hhmm = pd.to_datetime(info["start_time"], format="%H%M")
    end_hhmm = pd.to_datetime(info["end_time"], format="%H%M")

    recording_start = study_date + pd.Timedelta(
        hours=start_hhmm.hour, minutes=start_hhmm.minute
    )
    recording_end = study_date + pd.Timedelta(
        hours=end_hhmm.hour, minutes=end_hhmm.minute
    )

    if info["end_time"] == "2359":
        recording_end = study_date + pd.Timedelta(days=1) - pd.Timedelta(milliseconds=1)
    elif recording_end < recording_start:
        recording_end += pd.Timedelta(days=1)

    return {
        "serial": info["serial"],
        "study_date": study_date,
        "recording_start": recording_start,
        "recording_end": recording_end,
        "start_time_hhmm": info["start_time"],
        "end_time_hhmm": info["end_time"],
    }

def read_ecl_adc_counts(ecl_path: str | Path) -> tuple[np.ndarray, np.ndarray]:
    """ECL 16-bit wordからECG ADC countとevent codeを復号する。

    ECGは12-bit値を復号後、0x0800を引いてゼロ中心化する。
    mV換算は行わない。event codeは本モデル入力には使用しない。
    """
    ecl_path = Path(ecl_path)
    words_all = np.fromfile(ecl_path, dtype=ECL_DTYPE)

    extra = len(words_all) - EXPECTED_24H_SAMPLES_250
    if extra < 0:
        raise ValueError(
            f"ECL shorter than 24 h: actual={len(words_all):,}, "
            f"expected={EXPECTED_24H_SAMPLES_250:,}"
        )
    if extra > 0:
        # ECL入力仕様に合わせ、24時間相当を超えるtail wordは切り捨てる。
        words = words_all[:EXPECTED_24H_SAMPLES_250]
    else:
        words = words_all

    ecg_high4 = ((words & ECL_ECG_HIGH4_MASK) >> 4).astype(np.uint16)
    ecg_low8 = (words & ECL_ECG_LOW8_MASK).astype(np.uint16)
    raw12 = (ecg_high4 | ecg_low8).astype(np.int32)
    event_code = ((words & ECL_EVENT_MASK) >> 8).astype(np.uint8)
    ecg_counts = (raw12 - ECL_ZERO_LEVEL).astype(np.float32)
    return ecg_counts, event_code

def valid_range_250(source_info: dict, n_samples: int) -> tuple[int, int]:
    """ECL 24時間配列中の有効記録区間 [start,end) を250-Hz sampleで返す。"""
    file_start = source_info["study_date"]
    end_exclusive = source_info["recording_end"] + pd.Timedelta(milliseconds=1)

    start = int(round(
        (source_info["recording_start"] - file_start).total_seconds() * ORIG_FS
    ))
    end = int(np.ceil(
        (end_exclusive - file_start).total_seconds() * ORIG_FS
    ))

    start = max(0, start)
    end = min(int(n_samples), end)
    if end <= start:
        raise RuntimeError("Invalid valid recording range")
    return start, end

def bandpass_filter_for_ai(x_250: np.ndarray) -> np.ndarray:
    """連続250-Hz ECGへ0.3–100 Hz zero-phase Butterworth BPFを適用する。

    重要: window分割より前に、有効記録全体へ一度だけfilterする。
    """
    x = np.asarray(x_250, dtype=np.float32)
    if x.ndim != 1:
        raise ValueError(f"Expected 1-D ECG, got {x.shape}")
    if x.size < 32:
        raise ValueError("ECG is too short for sosfiltfilt")

    sos = butter(
        BPF_ORDER,
        [BPF_LOW_HZ, BPF_HIGH_HZ],
        btype="bandpass",
        fs=ORIG_FS,
        output="sos",
    )
    return np.asarray(sosfiltfilt(sos, x), dtype=np.float32)

def zscore_windows(x: np.ndarray, eps: float = 1e-6) -> np.ndarray:
    """20秒windowごとの外部z-score。stdはddof=0。"""
    x = np.asarray(x, dtype=np.float32)
    mean = x.mean(axis=1, keepdims=True)
    std = x.std(axis=1, keepdims=True)
    std = np.where(std < eps, 1.0, std)
    return ((x - mean) / std).astype(np.float32)

def build_ai_continuous_signal(
    ecg_all_250: np.ndarray,
    source_info: dict,
) -> tuple[np.ndarray, np.ndarray, int, int]:
    """ECLからAI用連続500-Hz波形とwindow開始位置を作る。

    Returns
    -------
    ecg_valid_500:
        BPF + resample後の有効記録波形。
    starts_abs_500:
        各20秒windowの、ECL日付0:00起点の絶対500-Hz sample位置。
    abs_valid_start_500, abs_valid_end_500:
        有効記録の500-Hz sample範囲。
    """
    valid_start_250, valid_end_250 = valid_range_250(source_info, len(ecg_all_250))
    ecg_valid_250 = np.asarray(
        ecg_all_250[valid_start_250:valid_end_250], dtype=np.float32
    )

    filtered_250 = bandpass_filter_for_ai(ecg_valid_250)
    ecg_valid_500 = resample_poly(
        filtered_250,
        up=FS,
        down=ORIG_FS,
    ).astype(np.float32)

    abs_valid_start_500 = valid_start_250 * UPSAMPLE_FACTOR
    abs_valid_end_500 = abs_valid_start_500 + len(ecg_valid_500)

    if len(ecg_valid_500) < WINDOW_SAMPLES:
        raise RuntimeError(f"Recording shorter than {WINDOW_SEC:.1f} s")

    starts_local = np.arange(
        0,
        len(ecg_valid_500) - WINDOW_SAMPLES + 1,
        STEP_SAMPLES,
        dtype=np.int64,
    )
    starts_abs_500 = abs_valid_start_500 + starts_local

    return (
        ecg_valid_500,
        starts_abs_500,
        int(abs_valid_start_500),
        int(abs_valid_end_500),
    )

def iter_ai_batches(
    ecg_valid_500: np.ndarray,
    starts_abs_500: np.ndarray,
    abs_valid_start_500: int,
    batch_size: int,
):
    """長時間記録で20秒windowを一括materializeしないためのbatch iterator。"""
    if batch_size < 1:
        raise ValueError("batch_size must be >= 1")

    starts_local = starts_abs_500 - int(abs_valid_start_500)
    for offset in range(0, len(starts_abs_500), batch_size):
        abs_batch = starts_abs_500[offset:offset + batch_size]
        local_batch = starts_local[offset:offset + batch_size]
        raw = np.stack(
            [ecg_valid_500[int(s):int(s) + WINDOW_SAMPLES] for s in local_batch],
            axis=0,
        ).astype(np.float32, copy=False)
        x = zscore_windows(raw)[..., None]
        yield offset, abs_batch, x
