# Brief: model-embedding

## Problem

検査会社へ配布する成果物に ONNX 等の生モデルが並ぶと、容易にコピー・流用される。  
ローカル（Windows）とクラウド（Linux）の両方で、モデル資産を守りながら推論を提供したい。

## Current State

- Phase-2 ONNX は `resources/models/` のファイルパスから `ort` でロードしている
- 大きな重みは Git 管理外。配布形態は未整備
- CLI `analyze-ecl` / `infer-window` がファイルパス前提

## Desired Outcome

- リリースバイナリにモデルが埋め込まれ、運用者が生 `.onnx` を扱わなくても推論できる
- 配布物に生モデルファイルを同梱しない（開発用パスロードは残してよい）
- Windows / Linux の release ビルドで同一方針が使える

## Approach

ビルド時にモデルバイト列を埋め込み（`include_bytes!` または同等）、実行時はメモリから ORT セッションを構築する。  
目標は「人間が読めない／簡単に持ち出せない」ことであり、熟練者によるリバース耐性の完全保証は範囲外とする。

## Scope

- **In**:
  - 埋め込み用のモデルロード経路（メモリ）
  - release / 配布プロファイルでの埋め込みビルド手順
  - 既存 CLI が埋め込みモデルで動くこと（パス指定は開発用に維持可）
- **Out**:
  - ライセンスサーバー
  - HTTP API・インストーラ本体
  - モデルの暗号化鍵ローテーション高度 DRM

## Boundary Candidates

- モデル資産のビルド注入とロード API
- 開発時ファイルロードとの切替（feature / 設定）

## Out of Boundary

- ライセンス確認
- API エンドポイント設計
- Docker / インストーラ

## Upstream / Downstream

- **Upstream**: 既存 `phase2` / `analyze` / ONNX export
- **Downstream**: `http-api`, `packaging-distribution`

## Existing Spec Touchpoints

- **Extends**: none（実装済みコアの強化）
- **Adjacent**: `runtime-ep-selection`（CPU/CUDA 選択は維持。埋め込みと独立）

## Constraints

- Git に本番モデルを置かない。CI／秘密ストアから bytes を注入
- バイナリ肥大（ONNX + ORT で 100MB 超になり得る）を許容し、計測する
- メモリロードは余分コピーを避ける API を優先検討
- 対象 OS: Windows / Linux x86_64
