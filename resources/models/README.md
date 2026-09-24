# Model resources

推論用のモデル重み・ONNX・関連メタデータを置くディレクトリです。  
CLI / 将来の API が実行時にここから読み込みます。

## 配置ファイル

| ファイル | Git | 用途 |
|---|---|---|
| `phase2_internal_finetuned_eventstrong_noise_20s10s_rev1.weights.h5` | 外 | Keras weight-only（学習成果） |
| `phase2_rev1.onnx` | 外 | Rust `ort` 用（`tools/export/export_onnx.py` で生成） |
| `phase2_rev1.onnx.json` | 内 | ONNX 入出力契約メタデータ |
| `README.md` | 内 | 本説明 |

## 生成方法

```bash
PYTHONPATH=tools python tools/export/export_onnx.py
```

詳細は `tools/export/README.md` を参照。

## 注意

- `*.h5` / `*.onnx` など大きなバイナリは Git 管理外です
- 配布・デプロイ時は別途パッケージング（CI artifact / オブジェクトストレージ等）で同梱してください
