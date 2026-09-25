# Brief: api-console-ui

## Problem

HTTP API だけでは、検査会社の担当者がブラウザから手早く疎通確認・単発解析しにくい。  
curl や専用クライアントなしに、導入検証と簡易運用ができる画面が必要である。

## Current State

- `http-api` 仕様は確定済み（解析 API・ヘルス等）。実装は `license-client` 完了後の予定に続き、その後本仕様を実装する
- 臨床向けの最終判定 UI・帳票・詳細波形ビューアはロードマップ範囲外
- 簡易コンソール UI の仕様・実装は未着手

## Desired Outcome

- ブラウザから ECL をアップロードして解析を実行できる
- 解析結果（JSON／CSV 等、API 契約に準拠）を画面表示またはダウンロードできる
- ヘルス状態を確認できる
- UI は HTTP API と**同一バイナリ／同一プロセス**で配信され、追加のフロントエンドサーバーを要しない
- ライセンス計上は既存 API／解析入口に委譲し、UI が独自に二重計上しない

## Approach

**簡単な仕組み**で足りる範囲に留める。

- 別のフロントエンドサーバーや SPA フレームワーク（React/Vue 等）は使わない
- HTTP API と同じバイナリが、埋め込みの静的ファイル（HTML + 最小の CSS/JS）を返す
- ブラウザの `fetch` / フォーム送信で既存 API（ヘルス・解析）を呼ぶだけ
- ビルド時にアセットをバイナリへ同梱（例: `rust-embed` または同等の単純な手段）

診断ワークステーション化・複雑な状態管理・ルーティングはしない。

## Scope

- **In**:
  - 静的コンソール UI（アップロード／実行／結果表示・DL／ヘルス）— HTML + 最小 JS のみ
  - API サーバーからの UI 配信ルート（例: `/` または `/ui`）
  - 必要最小の UX（エラー表示、処理中表示）
  - packaging 成果物への UI 同梱の受け渡し（本仕様がアセット契約を定義し、`packaging-distribution` は追従更新）
- **Out**:
  - 臨床最終判定 UI・波形エディタ・帳票
  - SPA フレームワーク／Node／別プロセスの UI サーバー
  - ライセンスサーバー・課金管理画面
  - 複雑 IAM（OAuth 製品化）
  - リッチなデザインシステム・多ページアプリ構成

## Boundary Candidates

- 静的アセットと配信ルート
- ブラウザ ↔ 既存 HTTP API の呼び出し（クライアント側）
- エラー／進捗の表示

## Out of Boundary

- 解析コア・モデル埋め込み・ライセンスゲート実装
- Docker／インストーラの本体設計（同梱パスの追記のみ隣接）

## Upstream / Downstream

- **Upstream**: `http-api`（エンドポイント契約）、`license-client`／`model-embedding`（間接）
- **Downstream**: `packaging-distribution`（UI 同梱の Existing Spec Update）

## Existing Spec Touchpoints

- **Extends**: `packaging-distribution`（成果物に UI を含める旨を後続で requirements 更新）
- **Adjacent**: `http-api`（ルート追加は本仕様が OWN、API 契約の再定義はしない）

## Constraints

- 実装開始は **`license-client` 実装完了後**（ロードマップ Phase 2）
- Windows／Linux 同一成果物方針
- 大きな ECL のサイズ上限・タイムアウトは `http-api` の設定に従う
- 言語・文言は製品仕様言語（日本語）に合わせる
