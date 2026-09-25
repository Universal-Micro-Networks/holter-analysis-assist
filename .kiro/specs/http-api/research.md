# Research & Design Decisions: http-api

## Summary
- **Feature**: `http-api`
- **Discovery Scope**: Extension（既存 lib / CLI 上に薄い HTTP 面を追加）
- **Key Findings**:
  - 解析・モデル・ライセンスは lib 側にあり、HTTP はルーティング／シリアライズ／起動ライフサイクルのみを所有する
  - プロジェクト MSRV 1.74 のため Axum は 0.7.x（0.7.9）を採用。Axum 0.8 系は MSRV 1.75+（最新は 1.80）で不適合
  - 上流 `license-client` は blocking HTTP、Axum は async のため、解析とゲート呼び出しは `spawn_blocking` で橋渡しする
  - 推論計上は `analyze` 入口の既存ゲートに委譲し、HTTP 層で二重計上しない

## Research Log

### 既存コードベースの拡張点
- **Context**: library-first / thin binary の第二面として HTTP を追加する
- **Sources Consulted**: `src/analyze.rs`, `src/main.rs`, `Cargo.toml`, `.kiro/steering/{product,tech,structure}.md`, upstream designs
- **Findings**:
  - `analyze_ecl_with_limit` が ECL→CSV/行結果の正本。HTTP はこれを呼ぶ
  - 現状 `[[bin]]` は CLI のみ。structure.md は将来 API バイナリ追加を想定
  - HTTP / ini / ライセンス層は未実装（上流仕様が提供予定）
- **Implications**: 新規 `holter-http-api` バイナリ + `src/http/` アダプタ。lib ドメインは解析ロジックを移さない

### Axum バージョンと MSRV
- **Context**: 候補フレームワーク Axum の選定とバージョン固定
- **Sources Consulted**: [crates.io/axum](https://crates.io/crates/axum), [axum CHANGELOG](https://docs.rs/crate/axum/latest/source/CHANGELOG.md), axum 0.7.7 docs（MSRV 1.66）
- **Findings**:
  - Axum 0.8.x: MSRV 1.75 → 1.78 → 1.80（現行最新）
  - Axum 0.7.9: MSRV 1.66 系で 1.74 と両立
  - Actix-web / Warp も候補だが、エコシステム・型安全ルーティング・メンテ観点で Axum が brief 推奨どおり最適
- **Implications**: `axum = "0.7.9"`（互換範囲は 0.7）、`tokio` multi-thread、`tower-http` で body limit / timeout

### 上流契約の整合
- **Context**: `ModelSource` / `LicenseGate` / `[license]` ini を再定義しない
- **Sources Consulted**: `.kiro/specs/model-embedding/design.md`, `.kiro/specs/license-client/design.md`
- **Findings**:
  - `ModelSource::{Path, Embedded}`、`Phase2Model::load_from_source`
  - `LicenseGate::{ensure_startup_licensed, ensure_inference_allowed}`、`LicenseConfig::load_from_path`、`[license]` キー一式
  - 1 推論 = analyze ジョブ / API 1 リクエスト。window 非計上
- **Implications**: HTTP は消費のみ。ini に `[http]` を追加し、`[license]` は上流キーをそのまま読む

### async / blocking 橋渡し
- **Context**: ライセンス client が blocking、解析パイプラインも同期
- **Findings**: Axum ハンドラから同期長時間処理を直接 await 内で回すとランタイムを塞ぐ
- **Implications**: 起動ゲートは listen 前に同期実行可。リクエスト解析は `tokio::task::spawn_blocking`（または同等）で実行

## Architecture Pattern Evaluation

| Option | Description | Strengths | Risks / Limitations | Notes |
|--------|-------------|-----------|---------------------|-------|
| Thin Axum adapter | バイナリ + 薄いハンドラが lib を呼ぶ | steering 準拠、二重実装なし | async/sync 橋が必要 | **採用** |
| Fat HTTP service in lib | ドメインに HTTP 型を持ち込む | 単一クレート集中 | library-first 違反、テスト肥大 | 却下 |
| Actix-web | 成熟した代替 | 実績 | brief 候補外、学習コスト | 却下（Axum で十分） |

## Design Decisions

### Decision: Axum 0.7.9 + Tokio
- **Context**: HTTP フレームワーク選定と MSRV 制約
- **Alternatives Considered**:
  1. Axum 0.8 — 最新だが MSRV 不適合
  2. Actix-web — 代替可能だが brief 推奨外
- **Selected Approach**: Axum 0.7.9 + Tokio multi-thread runtime
- **Rationale**: MSRV 1.74 を壊さず、薄いアダプタに適する
- **Trade-offs**: 0.8 の新 API は使えない。将来 MSRV 上げ時に 0.8 へ移行可能
- **Follow-up**: 実装時に `cargo +1.74 check` 相当で解決を確認

### Decision: 計上は analyze 入口に一元化
- **Context**: 二重計上リスク
- **Alternatives Considered**:
  1. HTTP ハンドラでも `ensure_inference_allowed` を呼ぶ
  2. analyze 入口のみ（上流契約どおり）
- **Selected Approach**: HTTP は meter を呼ばず、gated analyze エントリに委譲。起動ゲートのみ HTTP main が所有
- **Rationale**: 要件「1 リクエスト 1 回」と license-client 契約を一致させる
- **Trade-offs**: analyze 非経由の将来エンドポイント追加時は再配線が必要
- **Follow-up**: テストで meter 呼び出し回数 = 1 を検証

### Decision: multipart ECL + format 指定
- **Context**: 入出力契約
- **Alternatives Considered**:
  1. raw body only
  2. multipart `ecl` + `format=csv|json`
- **Selected Approach**: `POST /v1/analyze` で multipart フィールド `ecl` を主経路。任意クエリ/フォームで `format`（既定 csv）、`provider`、`max_windows`
- **Rationale**: ファイル名付きアップロードと将来拡張が容易
- **Trade-offs**: クライアントは multipart 必須。文書化で補う

### Decision: `[http]` ini セクション追加
- **Context**: リッスン・サイズ・タイムアウト・開発用 model_path
- **Selected Approach**: `[http]` に `bind`, `max_body_bytes`, `request_timeout_secs`, 任意 `model_path`, `provider`。`[license]` は上流そのまま
- **Rationale**: キー衝突を避け、Win/Linux 同一キーを維持
- **Trade-offs**: 設定ファイルが二セクションになる（意図的）

## Risks & Mitigations
- 長時間解析がワーカーを占有 — タイムアウト（既定 1800s）と body limit。同時実行数は初期はランタイム既定（将来必要なら Semaphore）
- 上流未実装時の結合不能 — モック ModelSource / LicenseGate で HTTP 単体を先行検証可能にする
- 一時ファイル寿命 — multipart を NamedTempFile に落として analyze に渡し、リクエスト終了で削除
- 秘密情報漏洩 — 上流マスク契約を踏襲。HTTP エラー JSON に api_key を載せない

## References
- [Axum crates.io](https://crates.io/crates/axum)
- Upstream: `.kiro/specs/model-embedding/design.md`, `.kiro/specs/license-client/design.md`
- Steering: `.kiro/steering/tech.md`, `structure.md`, `roadmap.md`
