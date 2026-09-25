# Brief: http-api

## Problem

クラウドや他システム連携では CLI だけでは組み込みにくい。  
同一の解析品質・ライセンス・モデル保護方針のまま、HTTP で解析を提供したい。

## Current State

- CLI（`analyze-ecl` 等）と `lib` のパイプラインは存在
- HTTP サーバー層は未実装（product / structure 上は「第二面」と記載済み）

## Desired Outcome

- ECL（または合意した入力）を受け取り、解析結果（CSV/JSON 等）を返す HTTP API
- CLI と同一コア（埋め込みモデル・ライセンスゲートを共有）
- 起動時ライセンス確認に失敗したらサーバーを立てない
- 各解析リクエストで 1 回ライセンス確認・計上する

## Approach

Rust HTTP フレームワーク（候補: Axum）で薄いサーバーバイナリを追加し、`lib` の解析 API を呼ぶ。  
認証はライセンスクライアントに委譲（アプリ利用者認証の本格 IAM は初期範囲を絞る）。設定は ini（リッスンアドレス、ライセンスセクション等）。

## Scope

- **In**:
  - 解析用 HTTP エンドポイント（入出力契約は requirements で確定）
  - ヘルスチェック等の最小運用面
  - ライセンスゲート接続（起動時 / リクエスト時）
  - 埋め込みモデルでの推論
- **Out**:
  - ライセンスサーバー
  - 複雑なマルチテナント IAM / OAuth 製品化（必要なら後続仕様）
  - Docker / インストーラ本体（呼び出し方の文書化は可）

## Boundary Candidates

- HTTP アダプタ（ルーティング・シリアライズ）
- ドメイン呼び出し（analyze）
- 設定・起動ライフサイクル

## Out of Boundary

- パッケージ形式の詳細
- モデル埋め込み実装そのもの（依存のみ）

## Upstream / Downstream

- **Upstream**: `model-embedding`, `license-client`, 既存 analyze パイプライン
- **Downstream**: `packaging-distribution`

## Existing Spec Touchpoints

- **Extends**: none（新規面）
- **Adjacent**: CLI（契約の二重管理を避ける）

## Constraints

- Windows / Linux で同一バイナリ用途（ローカル常駐 / クラウドプロセス）
- library-first: ビジネスロジックを HTTP 層に置かない
- 大きな ECL アップロードのタイムアウト・サイズ上限を requirements で決める
