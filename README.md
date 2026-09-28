# holter-analysis-assist

検査会社のホルター心電図解析業務を支援する、不整脈リズム分類アプリです。  
心電図（ECL）を入力として、拍ごとに **拍分類 `N` / `PAC` / `PVC`** と **リズム分類 `SR` / `AF/AFL`** を判定し、判定不能区間の `Unknown` フラグと連発（RUN）の `short_run_flag` を付けて出力します（詳細は「[出力形式](#出力形式csv--json)」）。

提供面:

| バイナリ | 役割 |
|---|---|
| `holter-analysis-assist` | CLI（ECL 解析・窓推論など） |
| `holter-http-api` | HTTP API + 同一プロセスの簡易ブラウザ UI（`/ui/`） |

どちらも同じ解析ライブラリを使い、起動時にライセンスの有効性を確認し、解析ジョブ単位で利用を記録します（`1 解析ジョブ = 1 計上`）。  
ライセンスサーバーへ到達できない／タイムアウト／成功以外の応答の場合は、起動または解析を **拒否** します（詳細は「[ライセンスサーバー接続](#ライセンスサーバー接続)」）。

## 前提

- Rust stable（`rust-toolchain.toml` で固定）。最低バージョン（MSRV）は **1.88**（依存の `ort` 2.0.0-rc.13 の要件）
- 対象ビルド: **Linux** (`x86_64-unknown-linux-gnu`) / **Windows** (`x86_64-pc-windows-msvc`)
- ライセンスサーバーの URL とライセンスキーは **ini** で指定（後述）。実行時はライセンスサーバーへの到達が必須

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

# CUDA なし（CPU のみ）
cargo build --release --no-default-features

# 配布向け（モデル埋め込み・CPU）。要: HOLTER_EMBEDDED_MODEL_PATH
# cargo build --release --bin holter-http-api --no-default-features --features embedded-model
```

Cargo 機能:

| 機能 | 既定 | 内容 |
|---|---|---|
| `cuda` | 有効 | ONNX Runtime の CUDA 実行プロバイダ（NVIDIA GPU）。外すと CPU 専用ビルドになり、`provider=cuda` は「利用不可」エラー、`auto` は CPU で動きます |
| `embedded-model` | 無効 | ビルド時に `HOLTER_EMBEDDED_MODEL_PATH` のモデルをバイナリへ埋め込む（配布用）。`--model` / `model_path` 未指定時に埋め込みモデルを使います |

## CLI の使い方

| サブコマンド | 内容 |
|---|---|
| `analyze-ecl <ECL>` | ECL 全体を解析して拍ごとの CSV を書く（本来の解析） |
| `compare-accel <ECL>...` | 推論高速化の設定を基準設定と比較する（「[比較サブコマンド](#比較サブコマンド-compare-accel)」） |
| `infer-window` | 前処理済み 1 窓（20 秒 @ 500 Hz）の ONNX 推論。モデル・実行プロバイダの動作確認用 |
| `serve-http --config <PATH>` | HTTP API + UI を起動（`holter-http-api` と同じ。後述） |
| `classify <INPUT>` | **スタブ**（疎通確認用）。ファイルが存在し空でないことだけを確認し、常に `NORMAL`・confidence 0.0 を返します。解析には `analyze-ecl` を使ってください |

```bash
# Full ECL pipeline（preprocess → ONNX → overlap postprocess → CSV）
cargo run --release -- analyze-ecl resources/samples/sample.ecl \
  --model resources/models/phase2_rev1.onnx \
  --output output/beat_results.csv

# Smoke（先頭 N window のみ）
cargo run --release -- analyze-ecl resources/samples/sample.ecl --max-windows 5

# Phase-2 ONNX reference（20s @ 500Hz window）
cargo run -- infer-window --model resources/models/phase2_rev1.onnx --format json

# Auto EP（CUDA → CPU）/ NVIDIA GPU（CUDA）
cargo run --release -- infer-window --provider auto --format json
cargo run --release -- infer-window --provider cuda --format json

# HTTP API + UI（holter-http-api と同じサーバー）
cargo run --release -- serve-http --config config/http.ini

# スタブ（常に NORMAL / 0.0）
cargo run -- classify path/to/any-file --format json
```

`analyze-ecl` のオプション:

| オプション | 既定 | 内容 |
|---|---|---|
| `--model <PATH>` | 埋め込みビルドは埋め込みモデル、それ以外は `resources/models/phase2_rev1.onnx`（カレントディレクトリ基準） | ONNX モデル |
| `--output <PATH>` | `output/beat_results.csv` | 出力 CSV。親フォルダは自動作成し、既存ファイルは上書き |
| `--max-windows <N>` | なし（全窓） | 先頭 N 窓だけ解析（スモーク用）。`0` も指定でき、その場合も利用は 1 件計上されます |
| `--provider` ほか | 「[推論の高速化設定](#推論の高速化設定)」 | 推論オプション |

成功時は標準出力に次の 5 行を出し、終了コード 0 で終わります。進捗（`[1/6] AI preprocessing ...` 〜 `[6/6] Final CSV ...`）と計測行（`perf: ...`）は標準エラーに出ます。

```text
saved: output/beat_results.csv
beats: <拍数>
windows: <解析した窓数>
Unknown=1: <Unknown=1 の拍数>
short_run_flag=1: <short_run_flag=1 の拍数>
```

失敗時は標準エラーに `error: <内容>` を出し、非 0 で終了します。

CLI のライセンス設定は `--license-config` または環境変数 `HOLTER_LICENSE_INI`（既定: `config/license.ini`）。サンプルは `config/license.ini.example`（キーは次節）。`serve-http` だけは例外で、`--config` の ini の `[license]` を使います（`--license-config` / `HOLTER_LICENSE_INI` は読みません）。

`infer-window` は前処理済み float32 LE 窓（40,000 bytes = 10,000 samples）を `--input` で渡せます。省略時は合成サイン波でスモークします。

### ECL 入力の条件

- **ファイル名**: `[10桁シリアル]_[yyyyMMdd]_[HHmm]_[HHmm].ecl`（例: `2501103675_20250512_1415_2359.ecl`）。日付は検査日、続く 2 つは記録の開始・終了時刻です。終了 `2359` はその日の終わりまで、終了が開始より前なら翌日にまたがる記録として扱います
- **中身**: 250 Hz・1 サンプル 2 バイト（リトルエンディアン）の列。サイズは偶数バイトで、**24 時間分（21,600,000 サンプル = 43,200,000 バイト）以上**が必要です。7 日分を超える部分は切り捨てます
- **解析範囲**: ファイル全体ではなく、ファイル名の開始〜終了時刻の範囲（最大 7 日）を 20 秒窓・17 秒刻みで解析します
- 条件を満たさない場合は推論前にエラーになり、利用は計上しません（HTTP は 400 `invalid_input`）

### 出力形式（CSV / JSON）

CSV（`analyze-ecl` の出力ファイル、HTTP の CSV 応答）は 1 行 1 拍で、列は次のとおりです。

| 列 | 内容 |
|---|---|
| `record_id` | ECL ファイル名（拡張子なし） |
| `beat_idx` | 0 始まりの拍番号 |
| `beat_time` | 拍の日時 `YYYY-MM-DD HH:MM:SS.mmm`（ECL の先頭を検査日の 0 時として換算） |
| `Unknown` | `1` = ノイズなどで判定不能な区間の拍、`0` = それ以外 |
| `beat_class` | `N` / `PAC` / `PVC`（`AF/AFL` 区間内の PAC は `N`） |
| `rhythm_class` | `SR` / `AF/AFL`。どの解析窓にも含まれない拍は `UNCOVERED` |
| `short_run_flag` | `1` = 連発（RUN）を構成する拍。`Unknown=1` の拍は常に `0` |

JSON（HTTP のみ）は集計と行の組です。`rows` の各要素は CSV と同じキーを持ちます。

```json
{"summary":{"beats":1,"unknown_ones":0,"short_run_ones":0,"windows":38},"rows":[{"record_id":"...","beat_idx":0,"beat_time":"...","Unknown":0,"beat_class":"N","rhythm_class":"SR","short_run_flag":0}]}
```

## ライセンスサーバー接続

CLI・HTTP API とも、ini の `[license]` セクションでライセンスサーバーを指定します（CLI は `--license-config` の ini、HTTP API と `serve-http` は `--config` の ini）。キーの意味の正本は `config/license.ini.example` です（他の設定サンプルや文書では再定義しません）。キー名は Windows / Linux 共通です。

```ini
[license]
server_url=https://license.example.com
license_key=lk_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
# timeout_secs=10
```

- `server_url`（必須）: ライセンスサーバーのベース URL。パス接頭辞付き（例: `https://host.example.com/license`）も可。エンドポイントのパスはサーバー契約で固定です。本番は `https://`、`http://` はローカル開発用
- `license_key`（必須）: 発行されたライセンスキー。未設定・空白のみは起動拒否。`Authorization: Bearer` ヘッダでのみ送信し、URL や要求本文には載せません。ログ・エラー出力にも値を出しません
- `timeout_secs`（任意）: ライセンス要求のタイムアウト秒数。既定 10
- `license_key` を書いた ini は秘密情報です。OS のファイル権限で所有者だけが読めるようにしてください（例: `chmod 600`、Windows は所有者のみの ACL）

動作:

- **起動時**: 有効性確認を 1 回行い、有効と応答された場合だけ起動します（HTTP API は失敗時にポートを開きません）
- **解析ジョブごと**: 推論の直前に利用を 1 件記録し、記録に成功した場合だけ解析します。心電図データや解析結果はサーバーへ送りません
- 計上するのは解析ジョブ（`analyze-ecl`、`compare-accel`、HTTP の `POST /v1/analyze`）だけです。`infer-window` と `classify` は起動時の有効性確認のみで、利用を記録しません
- 入力やモデルの不備（ECL のファイル名・内容、モデルの読み込み失敗）で推論に進めない場合は、利用回数を消費しません。推論を始めた後の失敗（推論エラー、出力の書き込み失敗、HTTP のタイムアウト）は消費済みになります。窓数 0（`max_windows=0`）でも 1 件計上します
- サーバーへ到達できない・タイムアウト・5xx など成功以外の応答は、すべて拒否します。オフライン運用はなく、自動リトライもしません
- 月間上限 0 のライセンスは **上限なし** として扱います。月の区切り（日本時間の暦月）と上限の判定はサーバー側で行います

拒否時の CLI は `error: license startup failed (<理由コード>): <説明>`（起動時）または `error: license inference denied (<理由コード>): <説明>`（解析時）を標準エラーに出し、非 0 で終了します。HTTP API は起動時の拒否を同じ形式で標準エラーに出して終了し、解析時の拒否はエラー応答（下表）で返します。

| 理由コード | 意味 | HTTP API（`POST /v1/analyze`） |
|---|---|---|
| `invalid_request` | キーの設定不備（欠落・形式不正。サーバーが HTTP 400 を返した場合） | 403 `license_inference_denied` |
| `license_invalid` | ライセンス無効（未登録のキー） | 403 `license_inference_denied` |
| `license_suspended` | 利用停止 | 403 `license_inference_denied` |
| `monthly_limit_reached` | 当月上限到達（解析時のみ） | 403 `license_inference_denied` |
| `rate_limited` | 要求過多（時間をおいて再試行可） | 429 `license_rate_limited` |
| `temporary_failure` | 一時障害・到達不能・タイムアウト（時間をおいて再試行可） | 503 `license_temporarily_unavailable` |
| `unexpected_response` | サーバー応答を解釈できない。接続先の不一致（404・405 など）もここに入り、`server_url` の確認を促す | 403 `license_inference_denied` |
| `gate_not_installed` | ライセンス確認の初期化前に解析が呼ばれた（内部エラー） | 403 `license_inference_denied` |

HTTP API のエラー本文の `error.message` は `<理由コード>: <説明>` の形です（例: `monthly_limit_reached: The monthly usage limit has been reached.`）。429 / 503 は時間をおいて再試行できる拒否、403 は設定やライセンス状態の見直しが必要な拒否です。

旧設定からの移行:

- `api_key` は `license_key` に書き換えてください（`api_key` だけの ini は設定エラーで起動しません。両方ある場合は `license_key` を使い、警告を 1 行出します）
- `check_path` / `meter_path` は廃止です。削除してください（書かれていると設定エラーで起動しません）

範囲外: 当月の利用状況（使用回数・残り回数）の照会・表示と、ライセンスの発行・停止などの管理用 API は本製品では扱いません。

## HTTP API / UI コンソールの使い方

`holter-http-api`（または CLI の `holter-analysis-assist serve-http`。起動するサーバーは同じ）は **1 プロセス**で次を提供します。

| 経路 | 内容 |
|---|---|
| `GET /health` | ヘルス。`200 {"status":"ok"}`（利用計上なし） |
| `POST /v1/analyze` | ECL 解析（multipart。詳細は「[5. 解析リクエストと応答](#5-解析リクエストと応答)」）。成功時 CSV または JSON |
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

# 任意。未指定時は埋め込みビルドなら埋め込みモデル、それ以外は
# resources/models/phase2_rev1.onnx（カレントディレクトリ基準）
# model_path=resources/models/phase2_rev1.onnx
# 任意。未指定時は auto（CUDA → CPU）
provider=cpu
# request_timeout_secs=1800
# max_body_bytes=536870912

[license]
server_url=https://license.example.com
license_key=lk_xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx
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

`[http]` と `[license]` を別ファイルで渡すことはできません。1 つの ini にまとめてください。

起動時の有効性確認に失敗した場合（拒否・サーバー未到達・タイムアウトを含む）は **ポートを開かず**終了します。  
解析リクエストのライセンス起因エラー（403 / 429 / 503）は「[ライセンスサーバー接続](#ライセンスサーバー接続)」を参照してください。

### 2. 起動

```bash
cargo build --bin holter-http-api
./target/debug/holter-http-api --config /path/to/merged.ini
# または
HOLTER_HTTP_INI=/path/to/merged.ini cargo run --bin holter-http-api
# holter-http-api を起動できない環境（Windows のアプリ制御など）では CLI から同じサーバーを起動
cargo run --release -- serve-http --config /path/to/merged.ini
```

起動時にモデルを読み込んで暖機推論を済ませ、ログに `listening on http://<bind>` が出れば受付開始です。モデルの読み込み・暖機に失敗した場合もポートを開かずに終了します。

### 3. UI コンソール（ブラウザ）

`bind` に書いたホスト:ポートで開きます。

```text
http://127.0.0.1:8080/ui/
```

画面は **左約 1/3 が入力・右約 2/3 が出力**（Bulma）。ヘルスは右上に控え目に表示され、**10 秒ごとに自動ポーリング**します（ボタンなし）。  
Bulma の CSS（1.0.2、MIT ライセンス。`static/console/vendor/bulma.min.css`、ライセンス文は `static/console/vendor/bulma.LICENSE`）はバイナリに同梱して `/ui/` から配信するため、外部 CDN への接続は不要です（閉域網でもそのまま表示できます）。  
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

本文サイズ上限・リクエストタイムアウトは ini の `max_body_bytes` / `request_timeout_secs`（既定 512 MiB / 1800 秒）に従います。

### 5. 解析リクエストと応答

`POST /v1/analyze` は `multipart/form-data` で次のフィールドを受け付けます。未知のフィールドは無視します。

| フィールド | 必須 | 内容 |
|---|---|---|
| `ecl` | ○ | ECL ファイル。**ファイル名が必須**で、「[ECL 入力の条件](#ecl-入力の条件)」の規約に従うこと。空のファイルは 400 |
| `format` | | `csv` / `json`（大文字小文字無視） |
| `provider` | | `auto` / `cpu` / `cuda`（ini の `provider` と同じ値）。常駐モデルと異なる実行プロバイダを指定すると、そのリクエストだけ一時セッションを作って解析します（`batch_size` と CUDA チューニングは起動時の設定を引き継ぎ、`auto` は常駐モデルを使います） |
| `max_windows` | | 先頭 N 窓だけ解析（0 以上の整数）。`0` でも利用は 1 件計上されます |

出力形式は **multipart の `format` > クエリ `?format=` > `Accept` ヘッダ（`application/json` / `text/csv`）> CSV（既定）** の順で決まります。`format` とクエリの値は `csv` / `json`（大文字小文字無視）で、それ以外は 400 `invalid_input` です。

成功時は `200` で、CSV（`text/csv; charset=utf-8`）または JSON（`application/json`）を返します（「[出力形式](#出力形式csv--json)」）。失敗時は原則として JSON `{"error":{"code":"<コード>","message":"<説明>"}}` を返します。ただし、ハンドラに入る前のミドルウェアやリクエストの取り出しの段階で拒否された場合（本文サイズ超過、multipart でない Content-Type など）は、JSON でない本文になることがあります。

| HTTP | `error.code` | 主な原因 |
|---|---|---|
| 400 | `invalid_input` | `ecl` フィールドやファイル名がない・空、ECL の規約違反、`format` / `provider` / `max_windows` の値が不正 |
| 413 | （JSON でない場合あり） | 本文が `max_body_bytes` を超えた。`Content-Length` 付きの通常の送信では、ハンドラに入る前に `text/plain` の `length limit exceeded` で返ります（JSON の `payload_too_large` になるとは限りません）。chunked 転送で超えた場合は 400 `invalid_input` になることがあります |
| 403 / 429 / 503 | `license_*` | ライセンスによる拒否（「[ライセンスサーバー接続](#ライセンスサーバー接続)」） |
| 504 | `request_timeout` | `request_timeout_secs` を超えた |
| 500 | `internal_error` | モデルの読み込み失敗・推論エラーなどサーバー側の失敗 |

504 を返した後も、解析はサーバー内で最後まで続きます。入力の検査を通過していれば利用は 1 件計上されます（結果は返りません）。ただし、常駐モデルの順番待ちのまま時間切れになった場合は、待ちが明けた時点で一時保存した ECL が消えているため読み込みで失敗し、計上されません。

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

`ort` のプリビルドは CUDA 13 版を使います。版の選択はビルド時の環境変数 `ORT_CUDA_VERSION`（既定 13）で行われ、実行時には影響しません。

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

CLI のオプション名は `[http]` の ini キーの `_` を `-` に置き換えたものです（Windows / Linux 共通）。`batch_size` と CUDA チューニングは CLI と ini で受け付ける値も同じです。`provider` だけは差があり、ini（と HTTP の `provider` フィールド）は大文字小文字を無視し別名 `gpu` / `nvidia`（= `cuda`）も受け付けますが、CLI の `--provider` は小文字の `auto` / `cpu` / `cuda` だけです。CLI では `analyze-ecl` と `compare-accel` で使えます（`infer-window` は `--provider` のみ）。

| CLI オプション | `[http]` キー | 値 | 未指定時 | 内容 |
|---|---|---|---|---|
| `--provider` | `provider` | `auto` / `cpu` / `cuda`（ini は上記の別名も可） | `auto`（CUDA → CPU） | 実行プロバイダ |
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
| `--max-windows <N>` | なし | ECL ごとのウィンドウ数の上限（1 以上。`analyze-ecl` と違い 0 は不可） |
| `--report-dir <DIR>` | `output/accel_compare` | 出力先（既存のファイルは上書き） |
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

- 別フォルダにある同名の ECL（大文字小文字の違いのみも含む）を並べた場合、2 件目以降のフォルダ名には `_2`、`_3`… が付きます。
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
resources/models/phase2_rev1.onnx                # tools/export/export_onnx.py で生成（固定バッチ 1）
resources/models/phase2_rev1.onnx.json           # 入出力契約（Git 管理）
resources/models/phase2_rev1_dynamic.onnx        # 可変バッチ版（任意。--dynamic-batch で生成）
resources/models/phase2_rev1_dynamic.onnx.json   # 可変バッチ版の入出力契約（Git 管理）
```

配布物（インストーラ・Docker イメージ・Linux パッケージ）はモデルを埋め込んだバイナリだけを含み、生のモデルファイルは同梱しません。

ONNX 生成:

```bash
python3 -m venv .venv-export && source .venv-export/bin/activate
pip install -r tools/export/requirements.txt
PYTHONPATH=tools python tools/export/export_onnx.py
```

## CI

GitHub Actions（`.github/workflows/ci.yml`）で Linux / Windows の  
`fmt` / `clippy` / `test` / `release` ビルドと成果物アップロードを行います。

埋め込みモデル用のリポジトリシークレット:

| シークレット | 内容 |
|---|---|
| `HOLTER_EMBEDDED_MODEL_URL` | 本番サイズの ONNX を取得する HTTPS URL（推奨） |
| `HOLTER_EMBEDDED_MODEL_AUTH_HEADER` | 任意。URL の取得時に付ける `Authorization` ヘッダの値 |
| `HOLTER_EMBEDDED_MODEL_B64` | 開発・フィクスチャ用の小さなモデルの Base64（シークレットの上限約 48 KB のため本番モデル不可。URL がある場合は無視） |

`ci.yml` のジョブ:

| ジョブ | 内容 |
|---|---|
| `build-test` | fmt / clippy / test / release ビルド、CLI バイナリのアップロード |
| `release-embedded-cli` / `release-embedded-http-api` | モデル埋め込み・CPU の CLI / `holter-http-api`（Linux / Windows） |
| `package-windows` / `package-linux` | `release-embedded-http-api` から Windows インストーラ、Docker イメージ（`docker save` アーカイブ）、deb / rpm を作成 |

`URL` / `B64` のどちらも未設定なら埋め込み・パッケージングのジョブはスキップされます（CI は失敗しません）。

GitHub Release 公開時（または手動実行）は `.github/workflows/release-packaging.yml` が  
Windows インストーラと Docker イメージ（`docker save` アーカイブ）を Artifact に残します。こちらはシークレット未設定なら失敗します。deb / rpm はこのワークフローでは作らず、`ci.yml` の `package-linux` で作ります。

## 開発フロー（cc-sdd）

Cursor Skills（`/kiro-*`）で仕様駆動開発します。次のステップ例:

```text
/kiro-steering
/kiro-discovery ホルター不整脈分類 CLI / HTTP API / 簡易 UI
```

## ライセンス

MIT
