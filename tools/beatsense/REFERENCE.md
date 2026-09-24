# BeatSense ホルター不整脈分類 — Python Reference Implementation

**Status:** 外部実装向け reference implementation（動作確認済み）  
**最終出力:** `beat_results.csv` のみ

## 1. この資料で最初に確認してほしいこと

本実装を他言語で再現する際に最も重要なのは、モデル内部の細かな実装よりも、**入力単位・出力粒度・クラス順・window分割方法・判定閾値**です。

### 1.1 入力単位

モデルへの1入力は、以下の **20秒の単一誘導ECG** です。

```text
sampling rate : 500 Hz
window length : 20 sec
samples       : 10,000
channel       : 1
layout        : NTC = (batch, time, channel)
dtype         : float32
shape         : (B, 10000, 1)
```

24時間波形をそのままモデルへ入力するのではなく、連続波形を20秒単位に分割して推論します。

### 1.2 20秒windowの分割

```text
window length = 20 sec
window overlap = 3 sec
window stride = 17 sec
```

例：

```text
window 0 :  0 sec 〜 20 sec
window 1 : 17 sec 〜 37 sec
window 2 : 34 sec 〜 54 sec
window 3 : 51 sec 〜 71 sec
...
```

したがって、隣接window間には常に3秒の重複があります。

window gridは記録開始から連続して作成し、clock-hour境界ではリセットしません。

### 1.3 モデル出力と判定粒度

通常の推論で使用する出力は3つです。

| output | shape | 粒度 | activation | 意味 |
|---|---|---|---|---|
| `beat` | `(B, 10000, 1)` | 500 Hz sample単位 | sigmoid | 各sampleのbeat probability |
| `event` | `(B, 10000, 3)` | 500 Hz sample単位 | independent sigmoid | `[PAC, PVC, N]` |
| `rhythm` | `(B, 1)` | 20秒window単位 | sigmoid | `SR` vs `AF/AFL` |

重要：`beat` と `event` は **sample-level output**、`rhythm` は **20秒window-level output** です。

### 1.4 EVENTクラス順

`event` のchannel順は固定です。

```text
channel 0 = PAC
channel 1 = PVC
channel 2 = N
```

`event` は3-class softmaxではなく、**3 channel independent sigmoid** です。

### 1.5 Rhythmの意味

```text
rhythm <  threshold → SR
rhythm >= threshold → AF/AFL
```

AFとAFLは別クラスではなく、1つのpositive classとして扱います。

### 1.6 最終出力の粒度

最終的に出力する `beat_results.csv` は、**1行 = 1 detected beat** です。

```text
record_id
beat_idx
beat_time
Unknown
beat_class
rhythm_class
short_run_flag
```

値の意味：

```text
beat_class
    N / PAC / PVC

rhythm_class
    SR / AF/AFL / UNCOVERED

Unknown
    0 / 1

short_run_flag
    0 / 1
```

UnknownとRUNはモデルの直接出力ではなく、後処理で付与します。

---

## 2. 全体処理フロー

```text
ECL 250 Hz
    ↓
ECL decode
    ↓
0.3–100 Hz band-pass filter
    ↓
250 Hz → 500 Hz resampling
    ↓
20 sec window / 3 sec overlap / 17 sec stride
    ↓
window z-score
    ↓
model input: (B, 10000, 1)
    ↓
┌─────────────────────────────────────┐
│ beat   : sample-level probability   │
│ event  : sample-level [PAC,PVC,N]   │
│ rhythm : 20-sec window-level score  │
└─────────────────────────────────────┘
    ↓
overlap-aware post-processing
    ↓
beat / PAC / PVC / SR / AF-AFL
    ↓
Unknown判定
    ↓
RRI RUN判定
    ↓
beat_results.csv
```

---

## 3. ファイル構成

```text
BeatSense_reference_v1.0.2/
├─ beatsense/
│  ├─ __init__.py
│  ├─ model.py
│  ├─ preprocess.py
│  ├─ postprocess.py
│  └─ analyzer.py
│
├─ sample_analyze_ecl.py
├─ README.md
├─ requirements.txt
├─ input/
├─ model/
└─ output/
```

### `model.py`

- `build_model()`
- weight-only H5のload
- 学習時と互換なKeras topology

### `preprocess.py`

- ECL decode
- valid range抽出
- band-pass filter
- 250→500 Hz resampling
- 20秒window / 3秒overlap
- z-score

### `postprocess.py`

- beat peak検出
- PAC/PVC/N判定
- overlap window統合
- rhythm timeline統合
- AF/AFL中PAC抑制
- Unknown
- RUN

### `analyzer.py`

上記を接続する上位処理です。

```python
from beatsense import analyze_ecl
```

### `sample_analyze_ecl.py`

入力ECL、weight、出力CSVを指定して実行するサンプルです。

---

## 4. 前処理仕様

### 4.1 ECL decode

ECLは16-bit little-endian wordとして読み込み、12-bit ECG値を復号します。

```text
ECG = decoded_12bit_value - 0x0800
```

モデル入力ではmV換算せず、ADC countを使用します。

### 4.2 Band-pass filter

```text
Butterworth band-pass
low  = 0.3 Hz
high = 100 Hz
order = 4
zero-phase = sosfiltfilt
sampling rate = 250 Hz
```

filterは各20秒windowごとではなく、**有効記録の連続波形に対してwindow分割前に適用**します。

### 4.3 Resampling

```text
250 Hz → 500 Hz
SciPy resample_poly
```

連続波形を500 Hzへ変換した後に20秒windowへ分割します。

### 4.4 Window z-score

各20秒windowについてtime axis方向に

```text
(x - mean) / std
```

を計算します。

学習時graph互換のため、`build_model()` 内にも `ZScoreNormalize1D` が存在します。外部window z-scoreとモデル内部z-scoreの両方を維持してください。

---

## 5. 判定閾値

reference implementationで使用する閾値は以下です。

```text
BEAT = 0.90
PAC  = 0.85
PVC  = 0.85
AF   = 0.85
```

EVENT判定：

```text
PAC >= 0.85 → PAC
PVC >= 0.85 → PVC
PAC/PVCが両方threshold以上 → scoreが高い方
それ以外 → N
```

Rhythm判定：

```text
rhythm <  0.85 → SR
rhythm >= 0.85 → AF/AFL
```

---

## 6. 3秒overlap推論の扱い

20秒windowは3秒重複するため、同じbeatや同じ時刻が複数windowから評価されます。

### Beat / Event

- 各windowで `beat` probabilityからpeakを検出
- window内位置を記録全体のabsolute sampleへ変換
- 80 ms以内の候補を同一QRSとして統合
- 最終QRS位置はcluster内で `beat score` が最大の候補
- EVENT scoreは、window端の影響を避けるため20秒window中央に最も近い候補を採用
- beat位置±40 msでEVENT anchorを探索

### Rhythm

`rhythm` は20秒windowごとに1値です。

重複3秒区間でscore平均は行わず、隣接windowの中心時刻の中点を境界として、各時刻を1つのwindowに割り当てます。

そのwindowのrhythm labelを該当区間へ割り当て、連続する同一labelをmergeして最終rhythm timelineを作成します。

AF/AFL区間内のPACは最終beat classでは抑制し、`N`として出力します。

---

## 7. `build_model()` とweight load

weight-only H5からモデルを復元するため、学習時と互換な `build_model()` を `beatsense/model.py` に含めています。

```python
from beatsense.model import build_model

model = build_model()
```

weight load：

```python
from beatsense.model import load_phase2_model

full_model, inference_model, weight_sha256 = load_phase2_model(
    "/path/to/model.weights.h5"
)
```

処理：

```text
build_model()
    ↓
学習時互換full graphを構築
    ↓
load_weights(..., skip_mismatch=False)
    ↓
beat / event / rhythm inference model
```

補助headはweight-only H5を正しく復元するためにfull graph内に残していますが、通常の推論I/Oとしては使用しません。

### モデル構造の概要

```text
Input (B,10000,1)
    ↓
ZScoreNormalize1D
    ↓
Inception-Residual Encoder
    10000 → 5000 → 2500 → 1250
    ↓
Dilated TCN
    dilation = 1,2,4,8,16,32,64
    ↓
    ├─ rhythm head
    ↓
U-Net decoder
    1250 → 2500 → 5000 → 10000
    ↓
    ├─ beat head
    └─ event head
```

詳細なlayer定義は `beatsense/model.py` をreferenceとしてください。

---

## 8. RUN

RUNはモデルの別headではなく、検出beatのRRIから後処理で判定します。

candidate onsetを `rr[i]` とすると、直前3本の連続RRIのみを使用します。

```text
reference RRIs = rr[i-3], rr[i-2], rr[i-1]
reference_RRI  = median(rr[i-3:i])
```

onsetで決めたreferenceはRUN全体で固定します。

```text
4–29 beats
    全member RR <= 0.85 × frozen reference_RRI
    かつ
    60 / mean(member RR) >= 100 bpm

30 beats以上
    全member RR <= 0.90 × frozen reference_RRI
    かつ
    60 / mean(member RR) >= 90 bpm
```

`short_run_flag=1` は、最終採用RUNに含まれる短縮RRIの終点beatに付与します。

---

## 9. Unknown

Unknownはモデル出力ではなく、frequency-domain QCです。

元の未フィルタ250-Hz ECLを使用します。

```text
各clock hourを独立処理
    ↓
10秒window / 3秒overlap / 7秒stride
    ↓
各10秒windowを独立に250→500 Hz resample
    ↓
Hanning + one-sided abs(FFT)
    ↓
0 < f <= 40 Hz のamplitude spectrum area
```

当該ECL 1日のwindow area中央値をreferenceとし、

```text
area < median × 0.20
```

をcore Unknownとします。

core Unknown区間をmerge後、±10秒拡張したものを最終Unknownとして使用します。

RUN候補が最終Unknownと1 sampleでも重なる場合、そのRUN全体を棄却します。

---

## 10. 最終CSV

出力ファイル：

```text
beat_results.csv
```

列：

```text
record_id
beat_idx
beat_time
Unknown
beat_class
rhythm_class
short_run_flag
```

例：

```csv
record_id,beat_idx,beat_time,Unknown,beat_class,rhythm_class,short_run_flag
2508201863_20260722_1220_2359,0,2026-07-22 12:20:00.842,0,N,SR,0
2508201863_20260722_1220_2359,1,2026-07-22 12:20:01.674,0,PAC,SR,0
```

保存する解析結果はこのCSVのみです。

---

## 11. サンプル実行

Google Drive上の例：

```text
MyDrive/
└── BeatSense/
    ├── beatsense/
    ├── input/
    │   └── 2508201863_20260722_1220_2359.ecl
    ├── model/
    │   └── model.weights.h5
    ├── output/
    ├── sample_analyze_ecl.py
    ├── README.md
    └── requirements.txt
```

`sample_analyze_ecl.py`：

```python
BASE_DIR = Path("/content/drive/MyDrive/BeatSense")

INPUT_ECL_PATH = BASE_DIR / "input" / "2508201863_20260722_1220_2359.ecl"
WEIGHTS_PATH = BASE_DIR / "model" / "model.weights.h5"
OUTPUT_CSV_PATH = BASE_DIR / "output" / "beat_results.csv"
```

実行：

```python
from google.colab import drive
drive.mount("/content/drive")
```

```python
%cd /content/drive/MyDrive/BeatSense
```

```python
!pip install -r requirements.txt
```

```python
!python sample_analyze_ecl.py
```