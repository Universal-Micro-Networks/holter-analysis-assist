# Brief: runtime-ep-selection

## Problem
開発・解析マシンは macOS（CoreML）と Windows（NVIDIA GPU）が混在する。現状は CoreML 向けの手動 `--provider` しかなく、この PC（RTX 4080）では加速できず、環境ごとにコマンドやビルドを覚え直す必要がある。

## Current State
- `ort =2.0.0-rc.13` + feature `coreml` 固定
- `ExecutionProviderKind` は `cpu` / `coreml` のみ
- CLI（`infer-window` / `analyze-ecl`）と `examples/bench_infer.rs` は明示指定前提（デフォルト CPU）
- CoreML は macOS/iOS 専用。Windows では利用不可

## Desired Outcome
実行時に利用可能な実行プロバイダを選び、可能な限り加速する。
- デフォルト `--provider auto`: 優先順位 **CUDA → CoreML → CPU**（失敗時は次へフォールバック）
- 明示指定 `cuda` / `coreml` / `cpu` も維持（ベンチ比較用）
- 選択結果（実際に使った EP）をログ / レポートに出す

## Approach
**Auto + 明示指定（案1）**
- `Phase2Model::load_with_provider` に `Auto` / `Cuda` を追加
- `auto` はコンパイル済み EP を優先順に試し、セッション生成に成功したものを採用
- Cargo は `lax-feature-matching` を使い、`cuda` / `coreml` はプロジェクトの optional features 経由で有効化（全ターゲット常時両 EP は避ける）
- CI（Linux 無 GPU）は EP なし、または CPU のみでビルド可能にする

## Scope
- **In**:
  - `ExecutionProviderKind` 拡張（`auto` / `cuda` / `cpu` / `coreml`）
  - ランタイム選択・フォールバック・実際に採用した EP の報告
  - CLI / bench / README 更新
  - Windows+NVIDIA での CUDA 経路の有効化手順
- **Out**:
  - TensorRT / DirectML / OpenVINO 等の追加 EP
  - モデル再エクスポートや精度チューニング
  - CUDA Toolkit / ドライバの自動インストール
  - HTTP API 層

## Boundary Candidates
- EP 解決ロジック（`phase2` 内のロード／フォールバック）
- CLI / example の `--provider` 公開面
- ビルド feature 設計（`cuda` / `coreml` / `lax-feature-matching`）とドキュメント

## Out of Boundary
- Python 比較ツールの全面書き換え（必要なら最小の provider 引数追加のみ）
- 前処理・後処理・分類ラベル契約の変更
- クラウド推論やリモート GPU

## Upstream / Downstream
- **Upstream**: 既存 Phase-2 ONNX（`resources/models/phase2_rev1.onnx`）、`ort` セッション構築
- **Downstream**: `analyze-ecl` 本番スループット、将来 API 推論、ベンチ比較

## Existing Spec Touchpoints
- **Extends**: なし（既存 `.kiro/specs/` は空）
- **Adjacent**: Phase-2 推論モジュール（`src/phase2.rs`）、CLI（`src/main.rs`）

## Constraints
- `ort` の CUDA プリビルドは **CUDA Toolkit ≥13.2 + cuDNN** を要求し得る。ドライバ表示 CUDA 13.1 だけでは不足の可能性あり → 実行時フォールバックで CPU へ落とす
- `cuda`+`coreml` を全ターゲット常時有効にするとプリビルド解決が失敗し得る → `lax-feature-matching` + optional features
- `ort` の MSRV（1.88 付近）と本リポの `rust-version = "1.74"` の不整合を設計時に確認・必要なら引き上げ
- ライセンス: `ort` は MIT/Apache-2.0。CUDA/cuDNN は NVIDIA 実行時依存（配布物に同梱しない）
