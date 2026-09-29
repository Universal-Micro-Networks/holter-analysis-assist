# Technology Stack

## Architecture

- **Library-first**: 解析ロジックは `holter_analysis_assist` クレート（`src/lib.rs` 配下のモジュール）に置き、CLI と HTTP API が共有する
- **CLI + HTTP の二面**: CLI（`clap`）と HTTP サーバー（`holter-http-api` バイナリ、`src/http/`）は実装済み。HTTP の起動処理は lib（`http::run_blocking`）にあり、CLI の `serve-http` からも同じサーバーを起動できる
- **正本解析入口**: 実推論は `analyze_ecl_with_source`（`ModelSource` → `Phase2Model` をロードして解析）。HTTP の常駐モデル経路は `analyze_ecl_with_model`。ライセンス計上はこれらの共通本体内でジョブあたり 1 回（前処理後・推論直前）
- **旧スタブ**: `classify` サブコマンド / `classify_ecg` は初期のスタブのまま残っている（実推論には使わない）

## Core Technologies

- **Language**: Rust (edition 2021, MSRV 1.88。`ort 2.0.0-rc.13` が 1.88、clap 4.6 が 1.85 を要求)
- **CLI**: clap 4 (derive, env)
- **HTTP**: axum 0.7（multipart）+ tokio（multi-thread）+ tower-http 0.5（body limit / timeout）。同期の解析は `spawn_blocking` で実行
- **License client**: reqwest 0.12（`blocking` + `rustls-tls`、非同期ランタイム不要）
- **Config**: rust-ini 0.21（`[license]` / `[http]` セクション）
- **Console UI**: rust-embed 8 + mime_guess（`static/console/` をバイナリへ埋め込み）
- **ONNX inference**: ort `=2.0.0-rc.13`（`lax-feature-matching`）+ ndarray。EP は実行時に `auto|cuda|cpu` を選択
- **Signal / output**: rustfft / regex / chrono / csv（前処理・ECL ファイル名解析・`beat_results.csv`）
- **Serialization**: serde / serde_json
- **Errors**: thiserror
- **Export tooling**: Python TensorFlow + tf2onnx (`tools/export/`)

### Cargo features
- `cuda`（**既定 on**）: ORT CUDA EP。実行時に CUDA Toolkit ≥13.2 + cuDNN が PATH 上に必要。埋め込み配布ビルド（CI の `release-embedded-*` / release-packaging）は `--no-default-features` で CPU のみ
- `embedded-model`（既定 off）: `build.rs` が `HOLTER_EMBEDDED_MODEL_PATH` のモデルを検証して埋め込む。未設定・空ファイルはビルド失敗

## Development Standards

### Type Safety
- 公開 API は明示的な型（`RhythmLabel`, `ClassificationResult`）
- `unwrap` / `expect` はテストと「到達不能」以外で避ける

### Code Quality
- `cargo fmt`
- `cargo clippy -- -D warnings`

### Testing
- ユニットテストは各モジュール内の `#[cfg(test)]`
- 結合テストは `tests/`（バイナリ起動は `assert_cmd`、HTTP ルータ単体は `tower::ServiceExt::oneshot`）
- ライセンスサーバーは実物を使わず、共有モック `tests/common/license_mock.rs` と lib 内 `MockLicenseClient` で検証する
- 実モデル不要の極小 ONNX フィクスチャ（`tests/fixtures/phase2_tiny_{dynamic,fixed1}.onnx`）をコミットし、実モデル・実 ECL 依存のテストは不在時 skip
- CI／配布の契約は `tools/check_*.sh` に置き、`tests/` から実行して固定する
- CI で Linux / Windows の両方を検証

## Development Environment

### Required Tools
- rustup + stable toolchain（`rust-toolchain.toml`）
- コンポーネント: rustfmt, clippy

### Common Commands
```bash
cargo build
cargo test
cargo clippy --all-targets -- -D warnings
cargo check --no-default-features                 # cuda なしでもビルドできること

# CLI（起動時にライセンス確認。既定 config/license.ini）
cargo run --release -- analyze-ecl <ECL> [--model PATH] [--provider auto|cuda|cpu] [--batch-size N]
cargo run --release -- infer-window [--model PATH] [--provider ...] [--format text|json]
cargo run --release -- compare-accel <ECL>... [推論オプション] [--report-dir DIR]
cargo run -- classify <INPUT> [--format text|json]   # 旧スタブ

# HTTP API + コンソール UI（[http] と [license] を含む ini。既定 config/http.ini）
cargo run --release --bin holter-http-api -- --config config/http.ini
cargo run --release -- serve-http --config config/http.ini   # 同じサーバーを CLI から

# 配布用（埋め込みモデル・CPU のみ）
HOLTER_EMBEDDED_MODEL_PATH=/path/to/model.onnx \
  cargo build --release --bin holter-http-api --no-default-features --features embedded-model
```

### Cross-platform targets
- Linux: `x86_64-unknown-linux-gnu`
- Windows: `x86_64-pc-windows-msvc`
- 成果物ビルドは GitHub Actions マトリクスを正とする（ローカルは開発ホスト）

## Key Technical Decisions

- ホスト全体への rustup 必須にせず、必要なら `$PWD/.cargo-tools` / `.rustup-tools` を利用可（gitignore 済み）
- モデル形式は ONNX（Phase-2、入力 `ecg [N,10000,1]`・出力 `beat` / `event` / `rhythm`）。入出力契約は `resources/models/phase2_rev1.onnx.json`。本番 `phase2_rev1.onnx` は固定バッチ 1 のため `batch_size`（既定 16）は警告付きで無視される。まとめ推論には可変バッチ版（`phase2_rev1_dynamic.onnx.json` のメタデータ）を使う
- 入力は ECL（250 Hz、16 bit LE）。ファイル名 `[serial]_[yyyyMMdd]_[HHmm]_[HHmm].ecl` から記録範囲を決める。24 時間未満はエラー、7 日（`MAX_RECORDING_DAYS`）を超える分は切り捨てて警告する
- 推論重みは `resources/models/` に置き、Git には含めない（配布は `embedded-model` でバイナリへ埋め込む）
- EP 選択は実行時: `auto` は CUDA → CPU の順に試す。ini / HTTP の `provider` は `gpu` / `nvidia` を `cuda` の別名として受け付ける。CoreML EP は精度劣化のため採用しない
- ライセンスは fail-closed（起動時確認に失敗したら CLI は実行しない。HTTP は受付を開始して画面・`/health` に警告を出すが、30 秒ごとの再確認が通るまで解析は利用記録なしで拒否する。オフライン回避なし）

---
_Document standards and patterns, not every dependency_
