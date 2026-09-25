# Research & Design Decisions

## Summary
- **Feature**: `license-client`
- **Discovery Scope**: Extension（既存 Rust lib / CLI へのゲート挿入）
- **Key Findings**:
  - 現行コードに ini / HTTP クライアントは未導入。`analyze_ecl_with_limit` が CLI・将来 API 共有の解析エントリである
  - プロジェクト MSRV は 1.74。`reqwest` 0.13 系は MSRV 1.85 のため不適合。0.12 系 + `blocking` が適合
  - ライセンスサーバー契約は別リポ合意前提。本仕様はクライアント側ポートと設定キーを固定し、パス等は ini で差し替え可能にする

## Research Log

### 既存コードの統合点
- **Context**: 起動時 / 推論時ゲートの挿入位置を特定する
- **Sources Consulted**: `src/main.rs`, `src/analyze.rs`, `src/lib.rs`, `Cargo.toml`, steering (`tech.md`, `structure.md`)
- **Findings**:
  - Library-first。解析本体は `analyze::analyze_ecl_with_limit`
  - CLI は `clap` サブコマンド。課金対象の「1 推論」は `AnalyzeEcl` のみ（`InferWindow` / `Classify` は window 単位・スタブ）
  - 設定・HTTP・ライセンスモジュールは未存在
  - エラーは `thiserror` パターンが既存
- **Implications**: 推論ゲートは `analyze` 入口に置き CLI/API 共有。起動ゲートはプロセス入口（`main`、将来は API バイナリ）に置く。`InferWindow` では計上しない

### HTTP クライアント選定（reqwest）
- **Context**: brief が `reqwest`（または同等）を提案
- **Sources Consulted**: crates.io reqwest versions / changelog、プロジェクト `rust-version = "1.74"`
- **Findings**:
  - `reqwest` 0.13.x: MSRV 1.85 → 採用不可
  - `reqwest` 0.12.x: MSRV 1.63 帯、blocking クライアントあり → CLI に適合（async runtime 不要）
- **Implications**: `reqwest = { version = "0.12", default-features = false, features = ["blocking", "json", "rustls-tls"] }` を採用方針とする（TLS 実装は設計で確定）

### INI パーサ選定
- **Context**: 設定は ini、Win/Linux 同一キー
- **Sources Consulted**: crates.io `rust-ini`, docs.rs `config` (config-rs)
- **Findings**:
  - `rust-ini`: INI 専用・軽量・読取に十分
  - `config-rs`: 多層設定向け。本要件（単一 ini）には過剰
- **Implications**: `rust-ini`（crate 名 `ini`）を採用

### サーバー契約の扱い
- **Context**: サーバーは別プロジェクト。パス・認証・レスポンスは合意前提
- **Sources Consulted**: brief.md, roadmap.md
- **Findings**: エンドポイント固定は本リポではできない。クライアントは抽象オペレーション（有効性確認 / 許可+計上）と ini による URL・認証・タイムアウト・任意パスを定義する
- **Implications**: ワイヤー契約は「合意可能な最小スキーマ」として design に置き、パスは設定で変更可能にする。契約変更は Revalidation Trigger

## Architecture Pattern Evaluation

| Option | Description | Strengths | Risks / Limitations | Notes |
|--------|-------------|-----------|---------------------|-------|
| Trait port + HTTP adapter | `LicenseClient` トレイト + reqwest 実装 | モック容易、テスト分離 | 薄い抽象が 1 実装のみ | Req 6 に直結。採用 |
| 具象 HTTP のみ | トレイト無しで直接 reqwest | 実装量少 | テストが結合寄り | 要件 6 に弱い |
| プロセス内カウンタのみ | サーバー無し | 単純 | 商用計上不可 | Out of scope |

## Design Decisions

### Decision: blocking reqwest 0.12 + rust-ini
- **Context**: CLI 同期フロー、MSRV 1.74、ini 必須
- **Alternatives Considered**:
  1. reqwest 0.13 async — MSRV 超過、tokio 追加が重い
  2. ureq — 軽いが brief 推奨とずれ、JSON 周りが薄い
  3. config-rs — 過剰
- **Selected Approach**: reqwest 0.12 blocking + rust-ini
- **Rationale**: MSRV / CLI / brief の三条件を満たす
- **Trade-offs**: 将来 API が async 化しても blocking 呼び出しは短いネットワーク待ちとして許容。必要なら後で async ポートを追加
- **Follow-up**: CI で Linux/Windows の依存解決を確認

### Decision: ゲートは lib の解析入口 + プロセス入口
- **Context**: CLI と将来 API が同一コアを共有（steering）
- **Alternatives Considered**:
  1. CLI のみにゲート — API で抜け道
  2. ONNX window ループ内 — 要件違反・過大計上
- **Selected Approach**: 起動=プロセス入口、推論=`analyze_ecl_with_limit` 入口で各 1 回
- **Rationale**: 「1 推論 = 1 ジョブ」を構造的に保証
- **Trade-offs**: 単体 window デバッグコマンドは計対象外（意図どおり）
- **Follow-up**: `http-api` 仕様は同じ Gate API を呼ぶこと

### Decision: サーバー経路は ini で差し替え、最小レスポンス契約のみ固定
- **Context**: サーバー別リポ
- **Alternatives Considered**:
  1. パスをハードコード — 合意変更でビルド必須
  2. 完全スキーマ未定義 — 実装不能
- **Selected Approach**: オペレーション契約（成功/拒否/通信失敗）と JSON 最小フィールドを固定。パス・ベース URL・API キーは ini
- **Rationale**: クライアント実装を進めつつサーバー合意に追従可能
- **Trade-offs**: サーバー側が追加フィールドを要求してもクライアントは無視可能に保つ
- **Follow-up**: 別リポとの契約文書リンクを運用で維持

## Synthesis Outcomes

### Generalization
- 起動確認と推論計上は同一 `LicenseClient` ポートの 2 メソッドとして一般化
- エラーは「起動失敗」と「推論拒否」の 2 区分に一般化（要件 5）

### Build vs Adopt
- HTTP: adopt reqwest（blocking）
- INI: adopt rust-ini
- ゲート論理・設定スキーマ: build（製品固有）

### Simplification
- リトライ/サーキットブレーカは初期スコープ外（fail-closed で十分）
- オフラインキャッシュ無し
- Mock 専用クレートは作らず、トレイト + テスト用スタブで足りる

## Risks & Mitigations
- サーバー契約未確定 — パスを ini 化し、最小 JSON 契約のみ固定。契約変更は revalidation
- 秘密情報のログ漏れ — Display/Debug で api_key をマスク。テストで検証
- 起動チェックが全サブコマンドに効く — InferWindow 開発時もサーバー到達が必要。開発用モックまたはテスト専用差し替えで緩和（本番は fail-closed）

## References
- [reqwest crates.io](https://crates.io/crates/reqwest) — バージョン / MSRV
- [rust-ini crates.io](https://crates.io/crates/rust-ini) — INI パーサ
- `.kiro/steering/tech.md`, `structure.md`, `roadmap.md`
- `.kiro/specs/license-client/brief.md`
