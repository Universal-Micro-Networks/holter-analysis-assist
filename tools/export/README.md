# ONNX export (BeatSense Phase-2 → Rust `ort`)

Keras weight-only H5（`*.weights.h5`）を、Rust 推論用 ONNX に変換します。

## 前提

- `tools/beatsense/model.py` の `build_model()` / `load_phase2_model()`
- 重み: `resources/models/phase2_internal_finetuned_eventstrong_noise_20s10s_rev1.weights.h5`

## 実行

```bash
cd /path/to/holter-analysis-assist
python3 -m venv .venv-export
source .venv-export/bin/activate
pip install -r tools/export/requirements.txt

PYTHONPATH=tools python tools/export/export_onnx.py \
  --weights resources/models/phase2_internal_finetuned_eventstrong_noise_20s10s_rev1.weights.h5 \
  --output resources/models/phase2_rev1.onnx
```

成功すると次が生成されます（いずれも gitignore 対象のバイナリ／メタ）:

- `resources/models/phase2_rev1.onnx`
- `resources/models/phase2_rev1.onnx.json`（入出力契約）

スクリプトは Keras と ONNX Runtime の出力を数値比較します。

## ランタイム I/O

| name | shape | 意味 |
|---|---|---|
| `ecg` (in) | `(B, 10000, 1)` | 20s @ 500Hz, NTC, float32 |
| `beat` | `(B, 10000, 1)` | sample-level sigmoid |
| `event` | `(B, 10000, 3)` | `[PAC, PVC, N]` independent sigmoid |
| `rhythm` | `(B, 1)` | window-level; high = AF/AFL |
