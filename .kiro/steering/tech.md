# Technology Stack

## Architecture

- **Library-first**: 分類ロジックは `holter_analysis_assist` クレート（`src/lib.rs`）に置き、CLI / 将来 API が共有する
- **CLI first → API later**: 当面は `clap` による CLI。HTTP 層は後続仕様で追加
- **Stub → Real inference**: `classify_ecg` は現在スタブ。入出力型を変えずに推論を差し替える

## Core Technologies

- **Language**: Rust (edition 2021, MSRV 1.74+)
- **CLI**: clap (derive)
- **ONNX inference**: ort `=2.0.0-rc.13` (+ ndarray); EP features `cuda` / `coreml` (default on) with runtime `--provider auto|cuda|coreml|cpu`
- **Serialization**: serde / serde_json
- **Errors**: thiserror
- **Export tooling**: Python TensorFlow + tf2onnx (`tools/export/`)

## Development Standards

### Type Safety
- 公開 API は明示的な型（`RhythmLabel`, `ClassificationResult`）
- `unwrap` / `expect` はテストと「到達不能」以外で避ける

### Code Quality
- `cargo fmt`
- `cargo clippy -- -D warnings`

### Testing
- ユニットテストは `src/lib.rs` 内
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
cargo run -- classify <INPUT> [--format text|json]
```

### Cross-platform targets
- Linux: `x86_64-unknown-linux-gnu`
- Windows: `x86_64-pc-windows-msvc`
- 成果物ビルドは GitHub Actions マトリクスを正とする（ローカルは開発ホスト）

## Key Technical Decisions

- ホスト全体への rustup 必須にせず、必要なら `$PWD/.cargo-tools` / `.rustup-tools` を利用可（gitignore 済み）
- モデル形式・ECG ファイルフォーマットは未確定 → 仕様フェーズ（`/kiro-discovery`）で確定する
- 推論重みは `resources/models/` に置き、Git には含めない（配布は別経路）

---
_Document standards and patterns, not every dependency_
