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

成功すると次が生成されます:

- `resources/models/phase2_rev1.onnx`（gitignore 対象のバイナリ）
- `resources/models/phase2_rev1.onnx.json`（入出力契約。`resources/models/*.onnx.json` は Git 管理対象）

スクリプトは Keras と ONNX Runtime の出力を数値比較します。

オプションを付けない場合は従来どおり **固定バッチ 1**（入力 `[1, 10000, 1]`）で出力します。`--batch-size N` で固定バッチ N を焼き込めます。

## 可変バッチでの再エクスポート（まとめ処理用）

複数ウィンドウを 1 回の推論でまとめて処理（`batch_size` > 1）するには、入力のバッチ次元を可変にしたモデルが必要です。固定バッチ 1 の旧モデルでも動作しますが、実効まとめ件数は 1 に落ち、警告が出るだけで高速化されません。

```bash
cd /path/to/holter-analysis-assist
source .venv-export/bin/activate

PYTHONPATH=tools python tools/export/export_onnx.py \
  --weights resources/models/phase2_internal_finetuned_eventstrong_noise_20s10s_rev1.weights.h5 \
  --output resources/models/phase2_rev1.onnx \
  --dynamic-batch \
  --verify-batch 4
```

| オプション | 意味 |
|---|---|
| `--dynamic-batch` | 入力バッチ次元を可変（`None`）にしてエクスポートする。`--batch-size` とは併用不可 |
| `--verify-batch N` | 検証に使うバッチ件数（既定 4、2 以上）。`--dynamic-batch` 指定時のみ有効 |

`--dynamic-batch` 指定時、スクリプトは次をすべて確認し、いずれかが失敗すると非 0 で終了します（`--skip-compare` で省略可能ですが推奨しません）:

1. ONNX Runtime から見た入力のバッチ次元が可変（シンボル次元）であること
2. バッチ N の入力で Keras と ONNX Runtime の出力が `--rtol` / `--atol` 以内で一致すること
3. バッチ N の各ウィンドウの ONNX 出力が、同じウィンドウをバッチ 1 で推論した出力と一致すること

メタデータ（`phase2_rev1.onnx.json`）には `"dynamic_batch": true` が記録され、入出力の `shape` のバッチ位置は `null` になります（例: `"shape": [null, 10000, 1]`）。

注意:

- **FP32 のみ**です。FP16 / INT8 への変換は対象外です。
- Rust 側はモデルの入力形状からバッチ次元が可変か固定かを **自動判定** します（可変なら設定したまとめ件数、固定 N なら N 件で推論）。メタデータ JSON を Rust が読むわけではないため、Rust 側の設定変更は不要です。
- モデル内部に固定バッチ前提の演算があると 1 または 3 の確認で失敗します。その場合は `--dynamic-batch` を外し、`--batch-size N` で固定バッチ N として出力してください（Rust 側は固定バッチ N も扱えます）。
- 再エクスポートしたモデルは、開発時は `resources/models/phase2_rev1.onnx` に置き、埋め込み配布ビルドでは `HOLTER_EMBEDDED_MODEL_PATH` で指定して埋め込みます。埋め込みビルドでは外部の生モデルファイルなしにまとめ処理を利用できます。

## テスト

引数処理とメタデータ生成は標準ライブラリだけで検証できます（TensorFlow 不要）:

```bash
python3 -m unittest tools/export/test_export_onnx.py
```

## ランタイム I/O

| name | shape | 意味 |
|---|---|---|
| `ecg` (in) | `(B, 10000, 1)` | 20s @ 500Hz, NTC, float32 |
| `beat` | `(B, 10000, 1)` | sample-level sigmoid |
| `event` | `(B, 10000, 3)` | `[PAC, PVC, N]` independent sigmoid |
| `rhythm` | `(B, 1)` | window-level; high = AF/AFL |
