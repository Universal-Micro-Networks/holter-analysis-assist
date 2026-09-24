# Project Structure

## Organization Philosophy

Library-first / thin binary。ドメインは `lib`、起動面は `main`（将来は `api` バイナリを追加）。

## Directory Patterns

### Library crate
**Location**: `src/lib.rs`（成長に応じて `src/` 配下へモジュール分割）  
**Purpose**: 分類ドメイン、エラー、入出力型  
**Example**: `classify_ecg`, `RhythmLabel`

### CLI binary
**Location**: `src/main.rs`  
**Purpose**: 引数解析と結果表示のみ。ビジネスロジックを置かない

### Spec-driven artifacts
**Location**: `.kiro/steering/`, `.kiro/specs/`  
**Purpose**: プロダクト方針と機能仕様

### CI
**Location**: `.github/workflows/ci.yml`  
**Purpose**: Linux / Windows の検証と release 成果物

### Runtime ML resources
**Location**: `resources/models/`  
**Purpose**: 推論用重み / ONNX など実行時リソース（大きなバイナリは gitignore）  
**Example**: `phase2_rev1.onnx`, `phase2_*.weights.h5`

### BeatSense Python reference
**Location**: `tools/beatsense/`  
**Purpose**: `build_model` / 前処理 / 後処理の正本（ONNX export と Rust 移植の参照）

### Phase-2 Rust inference
**Location**: `src/phase2.rs`  
**Purpose**: `ort` による window 推論（beat / event / rhythm）

### Preprocess / postprocess / analyze
**Location**: `src/preprocess.rs`, `src/postprocess.rs`, `src/analyze.rs`, `src/dsp.rs`  
**Purpose**: ECL→500Hz窓、overlap 統合、Unknown/RUN、`analyze-ecl` パイプライン

## Naming Conventions

- **Files / modules**: snake_case
- **Types**: PascalCase
- **Functions**: snake_case
- **Rhythm labels in wire format**: UPPERCASE (`NORMAL`, `AF`, `PAC`, `PVC`)

## Code Organization Principles

- CLI / API は `lib` に依存し、相互依存しない
- 推論実装の差し替えは `classify_ecg`（または後継トレイト）の内側に閉じる
- 新機能は cc-sdd 仕様（requirements → design → tasks）経由を基本とする

---
_Document patterns, not file trees. New files following patterns shouldn't require updates_
