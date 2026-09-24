# holter-analysis-assist

検査会社のホルター心電図解析業務を支援する、不整脈リズム分類アプリです。  
心電図を入力として **NORMAL / AF / PAC / PVC** を分類します。

現状は **CLI（スタブ推論）** です。同じライブラリから後続で API 化します。

## 前提

- Rust stable（`rust-toolchain.toml` で固定）
- 対象ビルド: **Linux** (`x86_64-unknown-linux-gnu`) / **Windows** (`x86_64-pc-windows-msvc`)

ローカルに rustup が無い場合、プロジェクト内ツールチェーンを使えます:

```bash
export CARGO_HOME="$PWD/.cargo-tools"
export RUSTUP_HOME="$PWD/.rustup-tools"
source "$CARGO_HOME/env"
```

## ビルド / 実行

```bash
cargo build
cargo test
cargo run -- classify path/to/ecg.bin
cargo run -- classify path/to/ecg.bin --format json

# Phase-2 ONNX reference（20s @ 500Hz window）
cargo run -- infer-window --model resources/models/phase2_rev1.onnx --format json

# Apple Silicon: CoreML（GPU / Neural Engine）
cargo run --release -- infer-window --provider coreml --format json

# Full ECL pipeline（preprocess → ONNX → overlap postprocess → CSV）
cargo run --release -- analyze-ecl resources/samples/sample.ecl \
  --model resources/models/phase2_rev1.onnx \
  --output output/beat_results.csv

# Smoke（先頭 N window のみ）
cargo run --release -- analyze-ecl resources/samples/sample.ecl --max-windows 5
```

`infer-window` は前処理済み float32 LE 窓（40,000 bytes = 10,000 samples）を `--input` で渡せます。省略時は合成サイン波でスモークします。

### Apple Silicon（CoreML）

`--provider coreml` で ONNX Runtime の CoreML EP（GPU / Neural Engine）を使えます。

- モデル形式は **NeuralNetwork**（`MLProgram` はこのモデルの AvgPool1D でコンパイル失敗するため）
- 初回ロードは CoreML コンパイルで数十秒かかることがあります（`resources/models/.coreml-cache/` にキャッシュ）
- この Phase-2 モデルでは CPU 比の推論高速化は限定的（~1.05x 程度）。ロードコストが大きいので短時間ジョブでは CPU の方が速いことがあります

CPU vs CoreML ベンチ（推論のみ）:

```bash
source .venv-export/bin/activate
PYTHONPATH=tools python tools/compare/bench_inference.py --max-windows 50
```

## モデルリソース

推論用重み / ONNX は `resources/models/` に配置します（バイナリは Git 管理外）。

```text
resources/models/phase2_internal_finetuned_eventstrong_noise_20s10s_rev1.weights.h5
resources/models/phase2_rev1.onnx          # tools/export/export_onnx.py で生成
resources/models/phase2_rev1.onnx.json     # 入出力契約（Git 管理）
```

ONNX 生成:

```bash
python3 -m venv .venv-export && source .venv-export/bin/activate
pip install -r tools/export/requirements.txt
PYTHONPATH=tools python tools/export/export_onnx.py
```

## CI

GitHub Actions（`.github/workflows/ci.yml`）で Linux / Windows の  
`fmt` / `clippy` / `test` / `release` ビルドと成果物アップロードを行います。

## 開発フロー（cc-sdd）

Cursor Skills（`/kiro-*`）で仕様駆動開発します。次のステップ例:

```text
/kiro-steering
/kiro-discovery ホルター不整脈分類 CLI → 後続 API
```

## ライセンス

MIT
