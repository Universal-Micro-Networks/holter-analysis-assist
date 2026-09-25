# Research & Design Decisions: api-console-ui

## Summary
- **Feature**: `api-console-ui`
- **Discovery Scope**: Extension（既存 `holter-http-api` / Axum ルータへの静的 UI 追加）
- **Key Findings**:
  - `src/http/routes.rs` は `/health` と `/v1/analyze` のみ。UI 配信ルートの追加は本仕様 OWN
  - 解析・ヘルス契約は `http-api` 正本。UI は相対パスで `fetch` / multipart するだけ
  - UI は `rust-embed` で同一バイナリへ同梱するため、packaging は別アセット配布ではなく「埋め込み済みバイナリに UI が含まれる」旨の追記が主

## Research Log

### 既存 HTTP 面の拡張点
- **Context**: 同一バイナリ配信の実装位置を特定する
- **Sources Consulted**: `src/http/routes.rs`, `src/http/handlers/*`, `.kiro/specs/http-api/design.md`, `Cargo.toml`（axum 0.7.9）
- **Findings**:
  - Router 組み立ては `build_router` に集約。ヘルス／解析ハンドラは分離済み
  - ボディ上限・タイムアウトは既存 middleware。UI 静的 GET にも同一層が掛かるが、アセットは小さく問題にならない
  - meter は正本入口内。UI 層から呼ばない
- **Implications**: `handlers/static_ui.rs`（または同等）＋ `routes.rs` へのマウント。analyze／health 契約は変更しない

### 埋め込み静的配信ライブラリ
- **Context**: SPA なし・同一バイナリ制約に合う手段を選ぶ
- **Sources Consulted**: [rust-embed docs.rs](https://docs.rs/rust-embed), [axum-embed](https://docs.rs/axum-embed), rust-embed axum example
- **Findings**:
  - `rust-embed` 8.x はフォルダをビルド時埋め込み。Axum では薄いハンドラで `Asset::get` + MIME 返却が公式例
  - `axum-embed` は便利だが追加依存。本要件の単一 HTML + 最小 JS/CSS には過剰
- **Implications**: `rust-embed` + 自前 `StaticUiHandler` を採用。`axum-embed` は非採用

### packaging 受け渡し
- **Context**: Downstream Existing Spec Update
- **Sources Consulted**: `.kiro/specs/packaging-distribution/*`, roadmap Existing Spec Updates
- **Findings**:
  - packaging 入力は既に埋め込み `holter-http-api`。ソース側 UI 埋め込み後は追加ファイル同梱が原則不要
  - 導入ドキュメントにコンソール URL（例: `/ui/`）を追記する必要がある
- **Implications**: 本仕様が配信経路を固定し、packaging は docs／スモーク観点を追従

## Architecture Pattern Evaluation

| Option | Description | Strengths | Risks / Limitations | Notes |
|--------|-------------|-----------|---------------------|-------|
| Embedded static + thin JS | rust-embed + HTML/JS | 同一バイナリ、依存少、要件一致 | UX は最小 | **採用** |
| SPA (React/Vue) + Node build | 別フロントビルド | 将来拡張 | 要件 Out、複雑化 | 却下 |
| 別静的ファイルサーバー | Nginx 等 | 分離容易 | 別プロセス必須で要件違反 | 却下 |
| axum-embed | ServeEmbed ラッパ | 実装短 | 追加クレート、過剰 | 非採用 |

## Design Decisions

### Decision: 配信経路は `/ui/`
- **Context**: brief は `/` または `/ui`
- **Alternatives Considered**:
  1. `/` に index のみ
  2. `/ui/` 配下に index + アセット、`/` は `/ui/` へリダイレクト
- **Selected Approach**: 正本は `/ui/`（および `/ui/index.html`）。利便のため `GET /` は `302`/`307` で `/ui/` へ誘導してよい
- **Rationale**: ヘルス／解析と明確に区別し、将来のルート衝突を避ける
- **Trade-offs**: ブックマークは `/ui/` 推奨
- **Follow-up**: packaging ドキュメントに URL を記載

### Decision: rust-embed + 最小ハンドラ
- **Context**: アセット同梱手段
- **Selected Approach**: `rust-embed`（フォルダ `static/console/`）と MIME 付き GET ハンドラ
- **Rationale**: 公式 Axum 例に沿い、依存を最小化
- **Trade-offs**: 圧縮・ETag 高度化はしない（不要）
- **Follow-up**: `mime_guess` を併用してよい

### Decision: クライアントは既存 API のみ呼ぶ
- **Context**: 二重計上禁止
- **Selected Approach**: JS から相対 URL で `GET /health`、`POST /v1/analyze`（multipart `ecl`、任意 `format`）
- **Rationale**: http-api 契約を再発明しない
- **Trade-offs**: CORS 不要（同一オリジン）
- **Follow-up**: 解析 1 回あたり meter 1 回は既存テスト契約を維持（UI 層に meter なし）

### Decision: packaging は「バイナリ内 UI」追記
- **Context**: Extends packaging-distribution
- **Selected Approach**: 別 UI アーティファクトは作らない。導入 docs／スモークに `/ui/` を追記する Existing Spec Update
- **Rationale**: 埋め込み後は配布入力が既に UI 込み
- **Follow-up**: packaging requirements/design/tasks を軽量更新

## Risks & Mitigations
- 大きな ECL でブラウザが固まる — 処理中表示＋上流タイムアウト／サイズ上限に委譲
- 静的ルートが body limit middleware に干渉 — GET 小サイズのみ；問題時は静的ルートを limit 外側に分離
- packaging ドキュメント未更新でオペレータが UI を知らない — Existing Spec Update タスクで明示

## References
- `.kiro/specs/http-api/design.md` — `/health`, `/v1/analyze`
- [rust-embed](https://docs.rs/rust-embed) — 埋め込み
- Roadmap Phase 2 — api-console-ui
