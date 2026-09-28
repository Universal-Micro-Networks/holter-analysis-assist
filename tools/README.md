# tools/

BeatSense reference（Python）、ONNX エクスポート、比較・計測用のツールと、CI / ビルド契約の確認スクリプトです。

| パス | 内容 |
|---|---|
| `beatsense/` | Phase-2 `build_model` / 前処理 / 後処理（外部実装向け reference） |
| `beatsense/REFERENCE.md` | BeatSense README（仕様正本） |
| `export/` | H5 → ONNX 変換スクリプト（[`export/README.md`](export/README.md)） |
| `compare/` | Python reference と Rust の比較・速度計測（`compare_pipelines.py`、`bench_inference.py`、`bench_fullday.py`、`run_full_ecl_bench.py`）と Windows 用ラッパ（`run_accel_compare.ps1` は `compare-accel` の実行。手順は [`docs/perf/windows-gpu-benchmark.md`](../docs/perf/windows-gpu-benchmark.md)、`run_cuda_bench.ps1` は `bench_infer` の CUDA 実行） |
| `check_embed_build_gate.sh` | `embedded-model` 機能のビルド条件（`HOLTER_EMBEDDED_MODEL_PATH` 必須）の確認 |
| `check_cli_embed_select.sh` | 埋め込みビルドの CLI のモデル選択の確認（ローカルのモック ライセンスサーバーを使用） |
| `check_ci_embed_release.sh` | `ci.yml` の `release-embedded-cli` ジョブの契約確認 |
| `check_ci_embed_http_release.sh` | `ci.yml` の `release-embedded-http-api` ジョブの契約確認 |
| `check_ci_packaging_release.sh` | `ci.yml` の `package-windows` / `package-linux` ジョブの契約確認 |

`compare/` の Python スクリプトはリポジトリ直下で `PYTHONPATH=tools` を付けて実行します（使い方は各スクリプト先頭の docstring）。

Rust ランタイムはリポジトリ直下の `src/` を参照してください。
