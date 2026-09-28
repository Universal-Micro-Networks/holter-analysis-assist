# Project Structure

## Organization Philosophy

Library-first / thin binary。ドメインは `lib`、起動面は CLI（`src/main.rs`）と HTTP API（`src/bin/holter_http_api.rs`）。どちらも lib の公開関数を呼ぶだけで、相互に import しない。

## Directory Patterns

### Library crate
**Location**: `src/lib.rs` + `src/` 配下のモジュール（大きいものは `src/<module>/` ディレクトリに分割）  
**Purpose**: 解析ドメイン、エラー、入出力型、HTTP アダプタ、ライセンスクライアント  
**Example**: `analyze_ecl_with_source`, `ModelSource`, `Phase2Model`, `LicenseGate`

### Binaries
**Location**: `src/main.rs`（CLI `holter-analysis-assist`）、`src/bin/`（`holter-http-api`）  
**Purpose**: 引数解析・起動順序・結果表示のみ。ビジネスロジックを置かない。HTTP の起動処理は lib の `http::run_blocking` にあり、CLI の `serve-http` も同じ関数を呼ぶ

### Analysis pipeline
**Location**: `src/preprocess.rs`, `src/dsp.rs`, `src/phase2.rs`（+ `src/phase2/`）, `src/postprocess.rs`, `src/analyze.rs`  
**Purpose**: ECL→500Hz 窓、`ort` による window 推論（beat / event / rhythm）、overlap 統合・Unknown/RUN、`analyze-ecl` パイプライン  
**Example**: `src/phase2/batch_plan.rs`（実効まとめ件数）、`src/phase2/cuda_graph.rs`（CUDA Graph 実行器）のように、推論の内部詳細は `phase2` のサブモジュールに閉じる

### Inference settings / model source / diagnostics
**Location**: `src/inference_options.rs`, `src/model_source.rs`, `src/embedded_model.rs`, `build.rs`, `src/perf.rs`  
**Purpose**: CLI・HTTP・比較ツール共通の推論設定型とパーサ、Path / Embedded のモデル解決（`embedded-model` feature 時のみ埋め込みバイト）、段階別計測

### Feature modules
**Location**: `src/http/`（config / state / routes / handlers / startup / assets）、`src/license/`（config / client / gate / http / types）、`src/accel_compare/`（metrics / report）  
**Purpose**: HTTP アダプタ、ライセンス確認・計上、CPU 基準との精度・速度比較。いずれも解析本体は `analyze` を呼び、ロジックを複製しない

### Console UI assets
**Location**: `static/console/`  
**Purpose**: 日本語の簡易コンソール（HTML / CSS / JS。CSS は `vendor/` に同梱した Bulma）。`rust-embed` でバイナリに埋め込み `/ui/` で配信する

### Config samples
**Location**: `config/`  
**Purpose**: `*.ini.example` がキー意味の正本（`[license]` は license-client、`[http]` は http-api）。実運用 ini は利用者が用意する

### Packaging / docs
**Location**: `packaging/`（docker / windows / linux / scripts / NOTICE）、`docs/`（packaging・perf・コンソール手順）  
**Purpose**: 埋め込み済みバイナリを入力にした配布成果物の組み立てと、運用・計測手順。`packaging/out/` は生成物

### Spec-driven artifacts
**Location**: `.kiro/steering/`, `.kiro/specs/`  
**Purpose**: プロダクト方針と機能仕様

### CI
**Location**: `.github/workflows/ci.yml`, `.github/workflows/release-packaging.yml`  
**Purpose**: Linux / Windows の検証、埋め込みバイナリ（`release-embedded-cli` / `release-embedded-http-api`）と配布物の生成。`release-packaging.yml` は GitHub Release 公開時に fail-closed で Windows インストーラと Docker イメージを作る

### Tests
**Location**: `tests/`（結合テスト）、`tests/common/`（共有ライセンスモック）、`tests/fixtures/`（極小 ONNX）  
**Purpose**: 起動面・配布スクリプト・CI 契約の検証。ユニットテストは各モジュール内

### Runtime ML resources
**Location**: `resources/models/`  
**Purpose**: 推論用重み / ONNX など実行時リソース（大きなバイナリは gitignore。`.onnx.json` のメタデータのみ管理）  
**Example**: `phase2_rev1.onnx`, `phase2_rev1.onnx.json`, `phase2_*.weights.h5`

### Tooling
**Location**: `tools/beatsense/`（BeatSense Python 参照実装）、`tools/export/`（ONNX export・フィクスチャ生成）、`tools/compare/`（Python / Rust 比較・計測スクリプト）、`tools/check_*.sh`（CI 契約チェック）  
**Purpose**: `build_model` / 前処理 / 後処理の正本（ONNX export と Rust 移植の参照）、ONNX 再エクスポート、計測。`examples/` は推論ベンチ

## Naming Conventions

- **Files / modules**: snake_case
- **Types**: PascalCase
- **Functions**: snake_case
- **Output labels（`beat_results.csv` / JSON）**: `beat_class` は `N` / `PAC` / `PVC`、`rhythm_class` は `SR` / `AF/AFL`（rhythm window に覆われない拍は `UNCOVERED`）、フラグ列は `Unknown` / `short_run_flag`（0/1）
- **旧スタブのラベル**: `NORMAL` / `AF` / `PAC` / `PVC`（`classify` / `RhythmLabel` のみ）
- **ini キー ↔ CLI オプション**: 推論設定は `[http]` キーの `_` を `-` に置き換えた名前（例: `batch_size` → `--batch-size`）

## Code Organization Principles

- CLI / API は `lib` に依存し、相互依存しない（共有する起動処理は lib に置く）
- 解析は正本入口 `analyze_ecl_with_source`（常駐モデルは `analyze_ecl_with_model`）経由。ライセンス計上を行う入口を増やさない
- 推論実装の差し替え・高速化は `Phase2Model` の内側に閉じ、出力契約（CSV / JSON）を変えない
- 新機能は cc-sdd 仕様（requirements → design → tasks）経由を基本とする

---
_Document patterns, not file trees. New files following patterns shouldn't require updates_
