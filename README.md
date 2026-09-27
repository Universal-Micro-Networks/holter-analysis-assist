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

## 推論の高速化設定

GPU（Windows・NVIDIA Ampere 世代以降を想定）での推論を速くするための設定です。次の 5 つで構成されます。

- **まとめ推論**: 複数の 20 秒ウィンドウを 1 回の推論でまとめて処理する（`batch_size`）
- **CUDA チューニング**: TF32 演算・Conv1D パディング・CUDA Graph（実行グラフの再利用）を個別に切り替える
- **HTTP 起動時の暖機**: モデルを読み込んで暖機推論を済ませてからリッスンする（設定不要）
- **段階別計測ログ**: 解析ごとに段階別の所要時間と実効設定を標準エラーへ 1 行で出す（設定不要）
- **比較サブコマンド `compare-accel`**: 同じ ECL を基準設定（CPU・FP32・まとめ 1）と候補設定で解析し、精度差と速度をレポートにする

設定を何も指定しなければ、解析結果の契約（CSV 列・JSON キー・ラベル体系）は従来と同じです。GPU がない環境でも、従来どおり起動・解析できます。

### 設定項目（CLI オプション / ini キー）

CLI のオプション名は `[http]` の ini キーの `_` を `-` に置き換えたもので、受け付ける値も同じです（Windows / Linux 共通）。CLI では `analyze-ecl` と `compare-accel` で使えます。

| CLI オプション | `[http]` キー | 値 | 未指定時 | 内容 |
|---|---|---|---|---|
| `--provider` | `provider` | `auto` / `cpu` / `cuda` | `auto`（CUDA → CPU） | 既存。意味・選択肢は変更なし |
| `--batch-size <N>` | `batch_size` | 整数 1〜256 | 16 | 1 回の推論で処理するウィンドウ数 |
| `--cuda-tf32 <SWITCH>` | `cuda_tf32` | `true` / `false` / `on` / `off` / `1` / `0`（大文字小文字無視） | ONNX Runtime 既定（TF32 有効） | TF32 演算を許可する |
| `--cuda-conv1d-pad-to-nc1d <SWITCH>` | `cuda_conv1d_pad_to_nc1d` | 同上 | ONNX Runtime 既定（無効） | Conv1D の入力を NC1D へパディングする |
| `--cuda-graph <SWITCH>` | `cuda_graph` | 同上 | ONNX Runtime 既定（無効） | CUDA Graph を捕捉して再生する |

- CUDA チューニングを指定しない場合、その項目は ONNX Runtime に渡しません（本機能の導入前と同じ挙動）。
- 不正な値は解析を開始せずにエラーになります。CLI はライセンス確認より前に引数エラー（終了コード 2）、HTTP は設定エラーで起動しません。
- 設定例と各キーの説明: `config/http.ini.example`

```bash
# CLI: CUDA・まとめ 16・Conv1D パディング有効
cargo run --release -- analyze-ecl resources/samples/sample.ecl \
  --provider cuda --batch-size 16 --cuda-conv1d-pad-to-nc1d on
```

```ini
[http]
bind=127.0.0.1:8080
provider=cuda
batch_size=16
cuda_conv1d_pad_to_nc1d=true
# cuda_tf32=false
# cuda_graph=true
```

### 有効な組み合わせ

- `batch_size` はどの実行プロバイダでも有効です。ただし実際のまとめ件数はモデルの入力形状で決まります（次節）。
- CUDA チューニング 3 項目は、解決後の実行プロバイダが CUDA のとき（`cuda`、または CUDA が使える環境での `auto`）だけ適用されます。3 項目は互いに独立に指定できます。CPU（`auto` から CPU に切り替わった場合を含む）では適用せず、`warning: CUDA tuning (...) not applied: execution provider is cpu` を出して解析を続けます。
- `cuda_graph` を有効にすると、毎回の推論は実効まとめ件数ちょうどの形状で実行されます（最後の端数は零埋めし、詰め物分の出力は捨てます）。
- `provider=auto` と `cuda_graph` 有効の組合せでは、CUDA のセッション生成時に CUDA Graph が使えないと分かった場合は CPU に切り替わります（標準エラーに `provider cuda unavailable (...); trying next ...`）。一方、暖機・初回推論での捕捉失敗は `auto` でもエラーになります（HTTP は起動しません）。明示的な `cuda` では、いずれの失敗もエラーです。エラーメッセージは `cuda_graph=false` での無効化を案内します。本番で有効にする前に、対象の GPU で `docs/perf/windows-gpu-benchmark.md` の手順で動作を確認してください。

### 可変バッチモデルへの再エクスポート

現在の本番モデル `phase2_rev1.onnx` は **固定バッチ 1** です。このモデルでは `batch_size` を指定しても（未指定の既定 16 でも）実効まとめ件数は 1 のままで、`warning: model has fixed batch 1; batch_size=16 is ignored` が出ます（解析自体は従来どおり動きます）。

まとめ推論で速くするには、入力のバッチ次元を可変にしたモデルへ再エクスポートします。手順は [`tools/export/README.md` の「可変バッチでの再エクスポート（まとめ処理用）」](tools/export/README.md) を参照してください（`export_onnx.py --dynamic-batch --verify-batch 4`）。再エクスポートしたモデルは、開発時は `resources/models/phase2_rev1.onnx` に置き、埋め込み配布ビルドでは `HOLTER_EMBEDDED_MODEL_PATH` で指定します。Rust 側はモデルの入力形状から可変か固定かを自動判定するので、追加の設定は要りません。

### 診断ログ（標準エラー）

計測値は標準エラーだけに出し、CSV / JSON の本体には含めません。

```text
# CLI analyze-ecl の解析成功時
perf: provider=cuda batch_size=16 model_batch=dynamic cuda_tuning=tf32=default,conv1d_pad_to_nc1d=on,cuda_graph=default windows=2064 model_load_ms=812.3 preprocess_ms=... inference_ms=... postprocess_ms=... output_ms=... total_ms=... windows_per_s=... windows_per_s_inference=...

# HTTP 起動時（暖機完了後、listening の前）
holter-http-api: model ready provider=cuda batch_size=16 model_batch=dynamic cuda_tuning=... warmup_ms=... source=...

# HTTP の解析リクエストごと
holter-http-api: perf: provider=cuda batch_size=16 ... windows_per_s_inference=... response_encode_ms=...
```

- `batch_size` は実効まとめ件数、`model_batch` はモデルのバッチ次元（`dynamic` または `fixed:N`）です。
- `cuda_tuning` は CUDA に適用したときだけ `tf32=…,conv1d_pad_to_nc1d=…,cuda_graph=…`（各 `on` / `off` / `default`）を表示し、CUDA 以外では `not_applied` です。
- 計測していない項目は `-` です（HTTP で常駐モデルを使う通常の解析では `model_load_ms` は `-`。リクエストで常駐モデルと異なる `provider` を指定し、その場でモデルを読み込んだ場合は数値が入ります）。

### 比較サブコマンド `compare-accel`

同じ ECL を基準設定（CPU・FP32・まとめ 1・CUDA チューニング未指定）と候補設定で解析し、精度差と速度を比べます。候補設定は `analyze-ecl` と同じ推論オプションで指定します。

```bash
cargo run --release -- compare-accel path/to/a.ecl path/to/b.ecl \
  --provider cuda --batch-size 16 --cuda-conv1d-pad-to-nc1d on \
  --report-dir output/accel_compare/cuda_b16_pad
```

| オプション | 既定 | 内容 |
|---|---|---|
| `<ECL>...` | （必須） | 比較する ECL（1 件以上） |
| `--model <PATH>` | 埋め込みモデル / `resources/models/phase2_rev1.onnx` | 基準・候補の両方で使うモデル |
| `--provider` ほか推論オプション | `analyze-ecl` と同じ | 候補設定 |
| `--tolerance-samples <N>` | 40（500 Hz で 80 ms） | 拍を対応付ける許容幅（サンプル） |
| `--prob-stride <N>` | 1 | beat / event の出力確率を N ウィンドウごとに比較する（rhythm は全ウィンドウ） |
| `--max-windows <N>` | なし | ECL ごとのウィンドウ数の上限 |
| `--report-dir <DIR>` | `output/accel_compare` | 出力先 |
| `--min-rhythm-window-agreement <RATE>` | なし | 閾値: リズム区間（窓単位）の一致率の下限（0〜1） |
| `--min-beat-match-rate <RATE>` | なし | 閾値: 対応拍の割合の下限（基準比・候補比の小さい方、0〜1） |
| `--min-beat-class-agreement <RATE>` | なし | 閾値: 対応拍のラベル一致率の下限（0〜1） |
| `--max-prob-abs-diff <X>` | なし | 閾値: 出力確率の最大絶対差（beat / event / rhythm の最大）の上限 |
| `--max-offset-samples <X>` | なし | 閾値: 対応拍の位置ずれ最大値（サンプル）の上限 |

出力（`--report-dir` の下）:

| パス | 内容 |
|---|---|
| `report.json` | 機械可読レポート（設定、ファイル別・全体集計の精度指標、段階別時間、ウィンドウ/秒、判定） |
| `report.md` | 人向け要約（標準出力にも同じ内容を表示） |
| `<ECL のファイル名（拡張子なし）>/baseline.csv`、`candidate.csv` | ECL ごとの基準・候補の解析結果 |

- **ライセンス計上**: 基準と候補をそれぞれ正規の解析経路で実行するため、**ECL あたり 2 回**計上されます（CLI 起動時の確認とは別）。
- **終了コード**: `0` = 比較完了（閾値なし、または指定した閾値をすべて満たす）、`1` = エラー（ライセンス、モデル／候補プロバイダが使えない、解析、入出力）、`2` = 指定した閾値のいずれかが不合格。引数エラーも clap の仕様で `2` になりますが、この場合は解析を始める前に終了し、レポートは書きません。
- **閾値**: 既定の閾値はありません。1 つも指定しなければ合否は判定しません。採否の閾値は計測結果を見て別途決めます（本機能では確定しません）。
- 候補の実行プロバイダが使えない場合（例: CUDA 未導入で `--provider cuda`）は、解析を始める前に `candidate provider unavailable: ...` で中止します（終了コード 1）。
- 丸 1 日を超える ECL では `--prob-stride` を大きくしてください（詳細は計測手順書）。

Windows GPU 端末での速度・精度の計測手順（CUDA のパス設定込みのラッパ `tools/compare/run_accel_compare.ps1` を含む）: [`docs/perf/windows-gpu-benchmark.md`](docs/perf/windows-gpu-benchmark.md)

### 範囲外・配布への影響

- 配布物（Windows インストーラ・Docker イメージ・Linux パッケージ）に新たなランタイムライブラリは同梱しません。GPU で使う場合の CUDA Toolkit / cuDNN は、従来どおり利用者の環境に導入します（前節「NVIDIA（CUDA）」）。
- TensorRT 実行プロバイダ、FP16 / INT8 など FP32 以外の演算精度、複数 GPU での分散実行（GPU は 0 番固定）は範囲外です。
- チューニング設定の採否閾値（合否基準）の確定は範囲外です。`compare-accel` の計測結果を見て別途決めます。

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
