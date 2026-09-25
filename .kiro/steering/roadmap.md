# Roadmap

## Overview

検査会社向けホルター不整脈 AI（NORMAL / AF / PAC / PVC）を、Windows ローカル運用と Linux クラウド運用の両方で安全に配布・運用できるようにする。  
既存の Rust 解析コア（ECL 前処理・ONNX 推論・後処理・CLI）を前提に、モデル流出対策、ライセンス連携、HTTP API、導入パッケージを段階的に追加する。

## Approach Decision

- **Chosen**: 4 仕様の縦割り（埋め込み → ライセンス → API → 配布）
- **Why**: 優先順が明確で、モデル保護と課金計上を API／配布の前提にできる。レビュー境界も独立しやすい
- **Rejected alternatives**:
  - API 先行: モデル保護・課金が後回しになり契約手戻りが起きやすい
  - 配布先行: 埋め込み・ライセンス未固定だと成果物の作り直しが増える

## Scope

- **In**:
  - Windows（ローカル）/ Linux（クラウド）での実行
  - モデルのビルド時埋め込み（生 ONNX の非配布）
  - ライセンスクライアント（起動時 + 1 推論ごと）。エンドポイント等は ini 設定
  - HTTP API 化（CLI と同一コア）
  - API アクセス用の簡易ブラウザ UI（同一バイナリ配信）
  - Docker Image / Windows Installer / Linux パッケージ配布
- **Out**:
  - ライセンスサーバー本体・課金 DB・管理 UI（別プロジェクト）
  - 完全な耐リバースエンジニアリング（casual 流出防止が目標）
  - 臨床診断の最終判定 UI・波形エディタ・帳票（本製品は解析補助）

## Constraints

- 対象: `x86_64-pc-windows-msvc` / `x86_64-unknown-linux-gnu`
- 言語・コア: Rust + `ort`（既存 Phase-2 パイプラインを維持）
- モデル: Git に重みを置かない。release ビルドは CI／秘密ストアから注入
- ライセンス「1 推論」= **1 解析ジョブ**（`analyze-ecl` 相当 / API 1 リクエスト）。ONNX window 単位ではない
- 配布ビルドは CUDA 同梱を必須にしない（CPU 既定、GPU は任意バリアント）
- 設定: ライセンスサーバー URL 等は **ini ファイル**
- **実装順メモ**: `api-console-ui` の実装は **`license-client` 実装完了後**

## Boundary Strategy

- **Why this split**: モデル保護・課金・API・配布・簡易 UI は責務が異なり、依存順が固定できる
- **Shared seams to watch**:
  - 埋め込み後のモデルロード API（メモリから `Session` 構築）
  - ライセンスゲートの挿入点（プロセス起動 / 解析エントリ）
  - API と CLI が同じ `lib` を呼ぶこと
  - 配布成果物に生モデルファイルを含めないこと
  - UI は既存 API を呼び、ライセンス計上を二重にしないこと

## Specs (dependency order)

### Phase 1 — 基盤（完了）

- [x] model-embedding -- ONNX 等をビルド時埋め込みし、配布物から生モデルを分離する。Dependencies: none
- [x] license-client -- 起動時と 1 推論ごとにライセンス確認・計上。ini でエンドポイント設定。Dependencies: none
- [x] http-api -- 解析コアを HTTP API 化（CLI と共有）。Dependencies: model-embedding, license-client
- [x] packaging-distribution -- Docker / Windows Installer / Linux パッケージ。Dependencies: http-api

### Phase 2 — 簡易 UI

- [x] api-console-ui -- API 疎通・単発解析用の簡易ブラウザ UI（同一バイナリ＋静的 HTML/JS。SPA なし）。Dependencies: http-api

## Existing Spec Updates

- [x] packaging-distribution -- `api-console-ui` アセット／ルートを配布成果物に同梱するよう追記。Dependencies: api-console-ui
