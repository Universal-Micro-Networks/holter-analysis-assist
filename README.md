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
