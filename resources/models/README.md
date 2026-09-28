# Model resources

推論用のモデル重み・ONNX・関連メタデータを置くディレクトリです。  
開発用（非埋め込み）ビルドの CLI / HTTP API は、モデルを指定しない場合 `phase2_rev1.onnx` をここから読み込みます。

## 配置ファイル

| ファイル | Git | 用途 |
|---|---|---|
| `phase2_internal_finetuned_eventstrong_noise_20s10s_rev1.weights.h5` | 外 | Keras weight-only（学習成果） |
| `phase2_rev1.onnx` | 外 | Rust `ort` 用（`tools/export/export_onnx.py` で生成。固定バッチ 1） |
| `phase2_rev1.onnx.json` | 内 | ONNX 入出力契約メタデータ |
| `phase2_rev1_dynamic.onnx` | 外 | 可変バッチ版（任意。`export_onnx.py --dynamic-batch` で生成。まとめ推論用） |
| `phase2_rev1_dynamic.onnx.json` | 内 | 可変バッチ版の入出力契約メタデータ |
| `README.md` | 内 | 本説明 |

## 生成方法

```bash
PYTHONPATH=tools python tools/export/export_onnx.py
```

詳細は `tools/export/README.md` を参照。

## 注意

- `*.h5` / `*.onnx` など大きなバイナリは Git 管理外です
- 配布物（Windows インストーラ・Docker イメージ・Linux パッケージ）には生のモデルファイルを同梱しません。ビルド時に `embedded-model` 機能（`HOLTER_EMBEDDED_MODEL_PATH`）でモデルをバイナリへ埋め込み、そのバイナリだけを配布します（パッケージングの検証は `.onnx` などを含む成果物を拒否します）
