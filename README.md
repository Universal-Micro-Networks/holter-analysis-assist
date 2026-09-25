# holter-analysis-assist

検査会社のホルター心電図解析業務を支援する、不整脈リズム分類アプリです。  
心電図を入力として **NORMAL / AF / PAC / PVC** を分類します。

提供面:

| バイナリ | 役割 |
|---|---|
| `holter-analysis-assist` | CLI（ECL 解析・窓推論など） |
| `holter-http-api` | HTTP API + 同一プロセスの簡易ブラウザ UI（`/ui/`） |

どちらも同じ解析ライブラリを使い、起動時／解析ジョブ単位でライセンス確認・計上します（`1 解析ジョブ = 1 計上`）。  
**暫定:** ライセンスサーバーへ到達できない／タイムアウト／非 2xx／不正 JSON の場合は **許可**します。JSON で明示的に `allowed: false` のときだけ拒否します（本番サーバー整備後に fail-closed へ戻す想定）。

## 前提

- Rust stable（`rust-toolchain.toml` で固定）
- 対象ビルド: **Linux** (`x86_64-unknown-linux-gnu`) / **Windows** (`x86_64-pc-windows-msvc`)
- ライセンスサーバー到達先は **ini** で指定（後述）

ローカルに rustup が無い場合、プロジェクト内ツールチェーンを使えます:

```bash
export CARGO_HOME="$PWD/.cargo-tools"
export RUSTUP_HOME="$PWD/.rustup-tools"
source "$CARGO_HOME/env"
```

## ビルド

```bash
cargo build
cargo test

# HTTP API / UI コンソール用バイナリ
cargo build --bin holter-http-api

# 配布向け（モデル埋め込み・CPU）。要: HOLTER_EMBEDDED_MODEL_PATH
# cargo build --release --bin holter-http-api --no-default-features --features embedded-model
```

## CLI の使い方

```bash
cargo run -- classify path/to/ecg.bin
cargo run -- classify path/to/ecg.bin --format json

# Phase-2 ONNX reference（20s @ 500Hz window）
cargo run -- infer-window --model resources/models/phase2_rev1.onnx --format json

# Auto EP（CUDA → CPU）
cargo run --release -- infer-window --provider auto --format json

# NVIDIA GPU（CUDA）
cargo run --release -- infer-window --provider cuda --format json

# Full ECL pipeline（preprocess → ONNX → overlap postprocess → CSV）
cargo run --release -- analyze-ecl resources/samples/sample.ecl \
  --model resources/models/phase2_rev1.onnx \
  --output output/beat_results.csv

# Smoke（先頭 N window のみ）
cargo run --release -- analyze-ecl resources/samples/sample.ecl --max-windows 5
```

CLI のライセンス設定は `--license-config` または環境変数 `HOLTER_LICENSE_INI`（既定: `config/license.ini`）。サンプルは `config/license.ini.example`。

`infer-window` は前処理済み float32 LE 窓（40,000 bytes = 10,000 samples）を `--input` で渡せます。省略時は合成サイン波でスモークします。

## HTTP API / UI コンソールの使い方

`holter-http-api` は **1 プロセス**で次を提供します。

| 経路 | 内容 |
|---|---|
| `GET /health` | ヘルス（利用計上なし） |
| `POST /v1/analyze` | ECL 解析（multipart フィールド名 `ecl`）。成功時 CSV または JSON |
| `GET /ui/` | 簡易ブラウザ UI（アップロード／ヘルス／結果表示・DL） |
| `GET /` | `/ui/` へリダイレクト |

### 1. 設定（ini）— ポートもここ

`[http]` と `[license]` を **1 ファイルにまとめた** ini を渡します。  
サンプル: `config/http.ini.example`（`[http]`）+ `config/license.ini.example`（`[license]`）をマージ。

**リッスンポートは `[http] bind` で変更します**（ホスト:ポート）。インスタンスごとに ini を分ければ、同じホストで複数ポートを並べられます。

```ini
[http]
# 必須。ここを変えればポートが変わる
bind=127.0.0.1:8080
# bind=127.0.0.1:18080
# bind=0.0.0.0:8080

# 開発時（非埋め込みビルド）はモデルパスを指定
model_path=resources/models/phase2_rev1.onnx
provider=cpu
# request_timeout_secs=1800
# max_body_bytes=536870912

[license]
server_url=https://license.example.com
# api_key=replace-me
```

例: 別ポートで 2 プロセス起動

```bash
# terminal A — 8080
./target/debug/holter-http-api --config /path/to/http-8080.ini

# terminal B — 18080（別 ini で bind=127.0.0.1:18080）
./target/debug/holter-http-api --config /path/to/http-18080.ini
```

設定パスの指定順:

1. `--config <PATH>`
2. 環境変数 `HOLTER_HTTP_INI`
3. 既定 `config/http.ini`

起動時にライセンスが明示拒否（`allowed: false`）の場合は **ポートを開かず**終了します。  
サーバー未到達などの通信失敗は暫定的に許可します（上記）。

### 2. 起動

```bash
cargo build --bin holter-http-api
./target/debug/holter-http-api --config /path/to/merged.ini
# または
HOLTER_HTTP_INI=/path/to/merged.ini cargo run --bin holter-http-api
```

ログに `listening on http://<bind>` が出れば受付開始です。

### 3. UI コンソール（ブラウザ）

`bind` に書いたホスト:ポートで開きます。

```text
http://127.0.0.1:8080/ui/
```

画面は **左約 1/3 が入力・右約 2/3 が出力**（Bulma）。ヘルスは右上に控え目に表示され、**10 秒ごとに自動ポーリング**します（ボタンなし）。  
出力形式の既定は **JSON** です。SPA や別フロントサーバーは不要で、UI は既存 API を呼ぶだけで **独自にライセンス計上しません**。

手動確認観点: `docs/console-ui-smoke.md`。配布導入: `docs/packaging/`。

### 4. API を curl で叩く例

```bash
# ヘルス（計上なし）
curl -sS http://127.0.0.1:8080/health

# 解析（JSON）。ファイル名は ECL 規約どおり
#   [10桁シリアル]_[yyyyMMdd]_[HHmm]_[HHmm].ecl
curl -sS -H 'Accept: application/json' \
  -F "ecl=@/path/to/2501103675_20250512_1415_2359.ecl;filename=2501103675_20250512_1415_2359.ecl" \
  http://127.0.0.1:8080/v1/analyze

# 解析（CSV）
curl -sS -H 'Accept: text/csv' \
  -F "ecl=@/path/to/2501103675_20250512_1415_2359.ecl;filename=2501103675_20250512_1415_2359.ecl" \
  -o beat_results.csv \
  http://127.0.0.1:8080/v1/analyze
```

クエリ `?format=json` / `?format=csv` でも形式を指定できます。  
本文サイズ上限・リクエストタイムアウトは ini の `max_body_bytes` / `request_timeout_secs`（既定 512 MiB / 1800 秒）に従います。

### Execution providers

デフォルトは `--provider auto`（優先順位: **CUDA → CPU**）。明示指定も可能です。

| Provider | 対象 | ランタイム要件 |
|---|---|---|
| `cuda` | NVIDIA GPU | CUDA Toolkit ≥13.2 + cuDNN ≥9.23（いずれも PATH） |
| `cpu` | 全環境 | 追加要件なし |
| `auto` | 全環境 | 上から順に試し、使えたものを採用 |

#### NVIDIA（CUDA）

ランタイム要件（このリポの `ort` プリビルド）:
- CUDA Toolkit **≥ 13.2**（例: `C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.4`）
- cuDNN **≥ 9.23**（例: `C:\Program Files\NVIDIA\CUDNN\v9.26\bin\13.4\x64` を PATH に）

PowerShell 例:

```powershell
$env:CUDA_PATH = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.4"
$env:PATH = "C:\Program Files\NVIDIA\CUDNN\v9.26\bin\13.4\x64;$env:CUDA_PATH\bin;$env:CUDA_PATH\bin\x64;$env:PATH"

# 本番モデルでの推論マイクロベンチ
cargo run --release --example bench_infer -- `
  resources/models/phase2_rev1.onnx path/to/windows cuda 3

# EP スモーク用の小さな ONNX（精度評価不可）
python tools/export/make_smoke_onnx.py
cargo run --release --example bench_infer -- `
  resources/models/phase2_smoke.onnx output/bench_windows cuda 3
```

CPU vs CUDA 比較（ECL から窓を切り出し）:

```bash
# Windows PowerShell 例
$env:PYTHONPATH="tools"
python tools/compare/bench_inference.py --max-windows 50
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

GitHub Release 公開時は `.github/workflows/release-packaging.yml` が  
Windows インストーラと Docker イメージ（`docker save` アーカイブ）を Artifact に残します（要: 埋め込みモデル用シークレット）。

## 開発フロー（cc-sdd）

Cursor Skills（`/kiro-*`）で仕様駆動開発します。次のステップ例:

```text
/kiro-steering
/kiro-discovery ホルター不整脈分類 CLI / HTTP API / 簡易 UI
```

## ライセンス

MIT
