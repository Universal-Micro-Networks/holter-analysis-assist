# Research & Design Decisions

## Summary
- **Feature**: `model-embedding`
- **Discovery Scope**: Extension（既存 Phase-2 / CLI のモデルロード経路拡張）
- **Key Findings**:
  - 現状は `Phase2Model::load*` → `SessionBuilder::commit_from_file` のみ。CLI / `analyze` はパス必須
  - `ort = 2.0.0-rc.13` は `commit_from_memory`（所有 `Session`）と `commit_from_memory_directly`（寿命付き `InMemorySession`）を提供
  - 本番 `.onnx` は既に `.gitignore`。CI の release 成果物はバイナリのみだが、モデル注入ステップは未整備

## Research Log

### 既存ロード経路
- **Context**: 埋め込み経路の差し込み点を特定する
- **Sources Consulted**: `src/phase2.rs`, `src/analyze.rs`, `src/main.rs`, `Cargo.toml`, `.github/workflows/ci.yml`, `.gitignore`
- **Findings**:
  - `Phase2Model` は `session: Session` と `model_path: PathBuf` を保持
  - `analyze_ecl*` / `infer-window` は `onnx_path: &Path` 前提
  - EP 選択（CPU/CUDA/auto）は `load_with_provider` 内で解決済み。本仕様は維持
  - CI は Linux/Windows x86_64 で `cargo build --release` 後にバイナリのみ artifact 化
- **Implications**: `ModelSource` 抽象とメモリ commit を `phase2` に追加し、`analyze` / CLI はソース切替のみ行う

### ort メモリロード API
- **Context**: 余分コピーを避ける API の採否
- **Sources Consulted**: [SessionBuilder docs](https://docs.rs/ort/latest/ort/session/builder/struct.SessionBuilder.html), [ort#71](https://github.com/pykeio/ort/issues/71)
- **Findings**:
  - `commit_from_memory(&[u8]) -> Session` は一般的な ONNX 向け。セッション構築時に内部コピーし得る
  - `commit_from_memory_directly` は主に `.ort` と `use_ort_model_bytes_directly`。戻り値は寿命付きで `Phase2Model` の所有モデルと相性が悪い
- **Implications**: 本番は `.onnx` 前提のため `commit_from_memory` を採用。静的 `include_bytes!` を入力とし、余分コピー回避は将来の `.ort` 移行時の最適化候補とする

### ビルド時注入パターン
- **Context**: Git に本番重みを置かず CI から埋め込む
- **Sources Consulted**: Cargo features / `build.rs` 慣行、現行 CI、`.gitignore` の `*.onnx` 除外
- **Findings**:
  - Cargo feature（例: `embedded-model`）+ `build.rs` で環境変数パスのモデルを `OUT_DIR` へコピーし `include_bytes!` するのが一般的
  - feature 有効時にパス未設定ならビルド失敗で欠落を検出できる
- **Implications**: 開発 default はパスロード。配布／CI release のみ feature を有効化

## Architecture Pattern Evaluation

| Option | Description | Strengths | Risks / Limitations | Notes |
|--------|-------------|-----------|---------------------|-------|
| Feature + include_bytes | ビルド時バイト埋め込み | 単純、配布に生ファイル不要 | バイナリ肥大、再ビルドでモデル差替 | 採用 |
| 実行時暗号化コンテナ | 別ファイルを復号ロード | 差替容易 | 鍵配布・DRM 範囲に近い | 範囲外 |
| commit_from_memory_directly | ゼロコピー志向 | ピークメモリ低減の可能性 | `.ort` / 寿命管理が複雑 | 将来候補 |

## Design Decisions

### Decision: ModelSource による二重経路
- **Context**: 埋め込みと開発用パスを両立する（要件 1, 3, 4）
- **Alternatives Considered**:
  1. 埋め込みのみ — 開発体験が悪化
  2. パスのみ + 配布時に一時展開 — 生ファイルが残る
- **Selected Approach**: `ModelSource::{Path, Embedded}` を lib 公開契約とし、CLI は feature に応じて既定ソースを選択
- **Rationale**: brief の「開発用パス維持」と「配布に生モデルなし」を同時に満たす
- **Trade-offs**: CLI 引数セマンティクスが feature 依存になる → ヘルプとエラーメッセージで明示
- **Follow-up**: 下流 `http-api` は同じ `ModelSource` / `Phase2Model` を利用

### Decision: commit_from_memory を採用
- **Context**: メモリから Session を構築する（要件 1.2）
- **Selected Approach**: `SessionBuilder::commit_from_memory`
- **Rationale**: 現行 `.onnx` と `Session` 所有モデルに適合。`directly` は型・フォーマット制約が大きい
- **Trade-offs**: ORT 内部コピーを許容（要件の「優先検討」は research で記録済み）

### Decision: Cargo feature `embedded-model` + CI 注入
- **Context**: Git 非コミットと欠落時ビルド失敗（要件 2.3, 5.x）
- **Selected Approach**: feature 有効時のみ `build.rs` が `HOLTER_EMBEDDED_MODEL_PATH` を要求。CI が secret/store からファイルを配置して release ビルド
- **Rationale**: ローカル開発は feature なしで現状維持。配布ビルドだけ注入必須
- **Trade-offs**: モデル差替は再ビルド必須

## Risks & Mitigations
- バイナリ 100MB 超 — CI でサイズ計測・artifact メタ記録（要件 6）
- 熟練者による抽出 — casual 防止が目標であることを要件・設計 Non-Goals に明記（要件 2.2）
- feature 組合せで CUDA 既定との衝突 — `embedded-model` は独立 feature。EP 選択は既存のまま
- 下流 packaging が resources を同梱しうる — 本仕様は配布成果物に生モデルを含めない契約を定義。実パッケージは `packaging-distribution` が再検証

## References
- [ort SessionBuilder](https://docs.rs/ort/latest/ort/session/builder/struct.SessionBuilder.html)
- [ort InMemorySession discussion #71](https://github.com/pykeio/ort/issues/71)
- `.kiro/steering/roadmap.md`, `.kiro/specs/model-embedding/brief.md`
