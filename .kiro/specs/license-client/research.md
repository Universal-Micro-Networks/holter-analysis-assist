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

## 改訂調査（2026-09-29）: 確定したサーバー契約への追従

- **Discovery Scope**: Extension（light）。既存の `src/license/` 実装と、別リポ `holter-analysis-assist-license-server` のクライアント向け実装を突き合わせた
- **Sources Consulted**: サーバー側 `src/license_server/http/{client_routes,responses,auth,dto,dependencies}.py`, `domain/{errors,license_key}.py`, `app.py`（エラーハンドラ）。本リポ `src/license/*.rs`, `src/http/error.rs`, `static/console/console.js`, 既存結合テスト 7 ファイル
- **Findings**:
  - エンドポイントは `POST /v1/licenses/verify`（200）、`POST /v1/usage`（201）、`GET /v1/usage/current`（200）。本文は読まれない。キーは `Authorization: Bearer`
  - 応答は `{"ok":true,"data":{...}}` / `{"ok":false,"error":{"code","message"}}`。ステータス対応は `responses.py` の表（400/401/403/403/429/503。管理系は 401 unauthorized・404 license_not_found）
  - 予期しない例外とリポジトリ障害はすべて 503 `temporary_failure`。Flask の HTTPException（404 など）は `invalid_request` コードで元のステータスを返す
  - エラー文言は固定の英文（`domain/errors.py`）で、他ライセンスの存在や内部情報を含まない → クライアントのメッセージにそのまま載せてよい
  - キー形式は `lk_[0-9a-f]{32}`。形式不正は `invalid_request`
  - 上限 0 は上限なし（`remaining: null`, `monthly_limit: 0`）
  - UI コンソールは `error.code` と `error.message` を連結表示するだけで、コード別の分岐はない
  - 既存の結合テストはそれぞれ独自のモックサーバー（暫定契約）を持っている
- **Implications**: アダプタの全面更新、エラー型への理由区分の追加、設定スキーマの変更、HTTP エラー対応付けの追加、テスト用モックの共通化が必要

### Decision: 理由区分はサーバーのエラーコードをそのまま使う
- **Context**: 要件 11 で拒否理由の識別が必要
- **Alternatives Considered**: (1) 日本語ラベルの独自区分 (2) サーバーコード + クライアント独自コード
- **Selected Approach**: (2)。`invalid_request` などサーバーの 6 コードに、`unexpected_response` と `gate_not_installed` を加える
- **Rationale**: サーバー文書・管理画面・ログと同じ語で切り分けられる。既存メッセージは英語で統一されている
- **Trade-offs**: 利用者向けの日本語説明は README で補う

### Decision: エンドポイントを ini で変更できないようにする（`check_path` / `meter_path` 廃止）
- **Context**: 初版は契約未確定のためパスを ini で上書き可能にしていた
- **Alternatives Considered**: (1) キー名を保ったまま既定値を変更 (2) 廃止してエラー (3) 廃止して黙って無視
- **Selected Approach**: (2)
- **Rationale**: 旧パスが残った ini のまま動かすと、サーバーは 404 を `invalid_request` コードで返すため「キー設定不備」と誤判定される。明示的なエラーの方が切り分けやすい。接頭辞付きの配置は `server_url` 側で表現できる（末尾スラッシュ正規化）
- **Trade-offs**: 旧 ini はそのままでは起動しない（`license_key` 必須化と同時なので、書き換えは 1 回で済む）

### Decision: 永続的な拒否は 403 のまま、再試行可能な拒否だけ 429 / 503 に分ける
- **Context**: 要件 11.7（API 呼出側が再試行可否を区別できる）
- **Alternatives Considered**: (1) 全理由に個別の HTTP コード・`error.code` (2) 再試行可否の 2 系統のみ分ける
- **Selected Approach**: (2)。403 `license_inference_denied` は既存互換で維持し、429 `license_rate_limited` と 503 `license_temporarily_unavailable` を追加。永続的な理由はメッセージ先頭の理由コードで識別
- **Rationale**: 既存クライアント・UI の互換を保ちつつ、再試行判断に必要な区別を HTTP レベルで提供
- **Trade-offs**: 永続的な理由の機械判定はメッセージ解析が必要（必要になれば `error` に `reason` フィールドを追加する）

### Decision: `invalid_request` は HTTP 400 のときだけキー設定不備とする
- **Context**: サーバーの `app.py` は Flask の HTTPException（パス違いの 404、メソッド違いの 405 など）も `invalid_request` コードで返し、ステータスだけを元の値にする。コード優先で判定すると、`server_url` の誤りが「キー設定不備」と表示される
- **Selected Approach**: `invalid_request` は 400 のときだけ `InvalidRequest`。それ以外はステータス規則（429 / 5xx / その他→`unexpected_response`）で判定し、説明文に `server_url` の確認を促す案内を付ける
- **Rationale**: キー起因の拒否（`auth.py` の `extract_license_key` は必ず 400）と接続先の設定ミスを切り分けられる

### Decision: 利用記録の自動リトライはしない
- **Rationale**: 応答を受け取れなかった場合にサーバー側で記録済みの可能性があり、再送は二重計上になりうる。要求過多・一時障害は呼出側の再試行に委ねる

### Decision: キー形式をクライアントで検証しない
- **Rationale**: 形式はサーバーの `invalid_request` で判定される。クライアントで複製すると形式変更時に追従が必要になる。未設定・空白のみだけを設定読込時に拒否する

## References
- [reqwest crates.io](https://crates.io/crates/reqwest) — バージョン / MSRV
- [rust-ini crates.io](https://crates.io/crates/rust-ini) — INI パーサ
- `.kiro/steering/tech.md`, `structure.md`, `roadmap.md`
- `.kiro/specs/license-client/brief.md`
