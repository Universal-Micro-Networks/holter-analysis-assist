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
