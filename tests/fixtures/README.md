# テスト用フィクスチャ

## 極小 Phase-2 ONNX（合成モデル）

**学習済み重みは含みません。** 手で決めた係数のアフィン変換と Sigmoid だけで構成した合成モデルで、解析精度とは無関係です。まとめ推論（複数ウィンドウの一括推論）の一致・順序・端数などを CI で検証するためだけに使います。

| ファイル | バッチ次元 | 用途 |
|---|---|---|
| `phase2_tiny_dynamic.onnx` | 可変（シンボル `batch`） | まとめ推論の検証 |
| `phase2_tiny_fixed1.onnx` | 固定 1 | 固定バッチモデルで実効件数が 1 に落ちることの検証 |

入出力名・形状は本番モデル（`resources/models/phase2_rev1.onnx.json`）と同じ規約です。

| 名前 | 形状 | 計算 |
|---|---|---|
| 入力 `ecg` | `(B, 10000, 1)` float32 | — |
| 出力 `beat` | `(B, 10000, 1)` | `Sigmoid(ecg * 1.5 + 0.25)` |
| 出力 `event` | `(B, 10000, 3)` | `Sigmoid(ecg * [0.8, -1.2, 2.0] + [-0.3, 0.1, 0.4])` |
| 出力 `rhythm` | `(B, 1)` | `Sigmoid(ReduceMean(ecg, サンプル軸) * 3.0 - 0.5)` |

出力は入力に依存する決定的な値で、バッチ要素ごとに独立に計算されます（まとめ推論の各出力は 1 件ずつの推論結果と一致します）。opset 17 / IR version 8。

## 再生成

生成済みファイルをコミットしているため、テスト実行に Python は不要です。再生成する場合のみ Python の `onnx` と `numpy` が必要です。

```bash
python3 -m venv .venv-onnx && .venv-onnx/bin/pip install onnx numpy
.venv-onnx/bin/python tools/export/make_tiny_batch_onnx.py
```

生成は決定的で、同じスクリプトからは同一バイト列のファイルが出力されます（onnx 1.23.0 で確認。onnx のバージョンによってはシリアライズ結果が変わる可能性があります）。読込み確認は `cargo test --test phase2_tiny_fixtures` で行えます。

`.gitignore` は `*.onnx` を除外していますが、`tests/fixtures/*.onnx` のみ例外として Git 管理下に置いています。
