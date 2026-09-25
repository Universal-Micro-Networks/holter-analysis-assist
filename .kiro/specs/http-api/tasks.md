# Implementation Plan

- [x] 1. Foundation: HTTP 依存とバイナリ骨格を用意する
- [x] 1.1 HTTP サーバー用のランタイム依存とバイナリ入口を追加する
  - Axum 0.7.x（MSRV 1.74 適合）、Tokio multi-thread、本文制限／タイムアウト用の tower-http を依存に追加する
  - CLI とは別の HTTP サーバーバイナリ入口を追加し、ライブラリの HTTP モジュールを公開できる状態にする
  - ライセンスサーバー・配布パッケージ・IAM・モデル埋め込み本体・ライセンスゲート本体は追加しない
  - 完了条件: 新依存込みでビルド解決でき、HTTP バイナリを起動すると（未配線でも）プロセスとして実行できる骨格がある
  - _Boundary: HttpStartup_
  - _Requirements: 9.2, 9.3, 10.1, 10.2, 10.3, 10.4_

- [ ] 2. Core: 設定・エラー・ヘルスの独立部品
- [ ] 2.1 (P) HTTP 用 ini 設定を読み取り既定上限を固定する
  - リッスン指定を必須とし、本文最大サイズ既定 512 MiB・端到端タイムアウト既定 1800 秒を適用する
  - サイズ／タイムアウト／任意のモデルパス・実行プロバイダを同一キー名で Win/Linux から上書きできる
  - `[http]` を本仕様の example で文書化する（`config/http.ini.example`、または上流 `license.ini.example` への `[http]` 追記）
  - ライセンス節は上流 `config/license.ini.example` の `[license]` 正本と同一キーで読める前提とし、キーを別名再定義しない
  - 必須欠落・不正値は起動拒否（fail-closed）になる
  - サンプル設定にキー意味と（必要なら）TLS 終端の運用注記を含める
  - 完了条件: 正当な ini から設定を構築でき、既定値が要件どおりであり、必須欠落時は明確な設定エラーになる
  - _Boundary: HttpIniConfig_
  - _Requirements: 6.1, 6.2, 6.3, 6.4, 6.5, 7.1, 7.3, 7.5, 9.1_

- [ ] 2.2 (P) 失敗区分と CSV／JSON 応答変換を用意する
  - 入力不正・ライセンス推論拒否・過大ボディ・タイムアウト・内部エラーを呼出側が区別できる応答に写像する
  - 成功時は CLI と同等ラベルの CSV および機械可読 JSON（要約＋行）を出せる
  - エラー本文に秘密情報の平文を含めない
  - 完了条件: 代表的な失敗入力に対して区分付き応答が得られ、成功行のフィールドが CLI 相当と一致する
  - _Boundary: HttpError, ResponseCodec_
  - _Requirements: 1.2, 1.4, 8.1, 8.2, 8.3, 8.4_

- [ ] 2.3 (P) ヘルスチェック経路を追加する
  - 解析経路と区別できる経路で、受付可能状態を示す成功応答を返す
  - 利用計上を行わない
  - 完了条件: ヘルス経路への問い合わせが成功し、計上処理が呼ばれない
  - _Boundary: HealthHandler_
  - _Requirements: 5.1, 5.2, 5.3_

- [ ] 3. Core: 解析 HTTP アダプタ
- [ ] 3.1 ECL 解析リクエストを正本ライブラリ入口へ委譲する
  - multipart 等で ECL を受け取り、ビジネスロジックをアダプタに複製せず **正本 `analyze_ecl_with_source` のみ**を呼ぶ（並列 gated 入口を作らない）
  - 埋め込みモデルまたは設定上のパス指定モデルを上流の `ModelSource` 契約で選び、契約を再定義しない
  - AppState は ModelSource／制限値等を保持してよいが、analyze 用の別 `LicenseGate` を所有しない（meter は正本入口の global gate）
  - 同期の長時間解析は非同期ランタイムを塞がない方法で実行する
  - 入力不正時は解析せずクライアントエラーを返す
  - 本文サイズ超過は解析開始前に拒否する
  - アダプタ自身では `ensure_inference_allowed` / meter を呼ばない（正本入口内の 1 回に委譲。二重計上防止）
  - 完了条件: 有効 ECL で CSV／JSON 成功応答が返り、不正入力・過大ボディが失敗し、アダプタ経由の計上が二重にならない設計どおりである
  - _Boundary: AnalyzeHandler, AppState_
  - _Depends: 2.1, 2.2_
  - _Requirements: 1.1, 1.2, 1.3, 1.4, 2.1, 2.2, 2.3, 2.4, 4.1, 4.4, 4.5, 7.2, 9.2_

- [ ] 4. Integration: 起動ゲートとルーティング配線
- [ ] 4.1 起動時ライセンス確認後にのみリッスンしルートを公開する
  - 設定読取 → `LicenseGate::install` → `ensure_startup_licensed` → 共有状態構築 → バインドの順で起動する（CLI と同一の process-wide install）
  - 起動確認失敗・設定不備時はポートを開かず、起動失敗として識別可能に終了する
  - ヘルスと解析ルートを登録し、本文制限・リクエストタイムアウトを設定値で適用する
  - 正本入口の許可確認失敗は推論拒否応答に写像し、結果ボディを返さない
  - タイムアウト超過を処理失敗として表面化する
  - 完了条件: 起動ゲート成功時のみヘルス／解析が到達可能で、失敗時は非リッスン。解析 1 リクエストにつき計上 1 回（モック検証可能な状態。HTTP 層の追加 meter なし）
  - _Boundary: HttpStartup, AppState, AnalyzeHandler, HealthHandler_
  - _Depends: 1.1, 2.1, 2.2, 2.3, 3.1_
  - _Requirements: 3.1, 3.2, 3.3, 3.4, 4.1, 4.2, 4.3, 4.4, 4.5, 5.1, 7.4, 8.2, 9.1, 9.3_

- [ ] 4.2 CI ジョブ `release-embedded-http-api` で埋め込み HTTP バイナリを出す
  - `embedded-model` feature 付きで `holter-http-api` を Win/Linux 向けに release ビルドする
  - ジョブ／アーティファクト名を `release-embedded-http-api` とし、下流 packaging-distribution の入力とする
  - 生モデル（`.onnx` 等）を artifact に含めない。`release-embedded-cli` は変更しない
  - 完了条件: 両 OS（または合意マトリクス）の埋め込み `holter-http-api` artifact が得られ、packaging が参照できる
  - _Boundary: CiEmbedHttpRelease_
  - _Depends: 1.1_
  - _Requirements: 2.2, 11.1, 11.2, 11.3_

- [ ] 5. Validation: 契約と fail-closed の自動検証
- [ ] 5.1 設定既定値・エラー区分・ヘルス非計上の単体テストを追加する
  - 512 MiB／1800 秒既定、必須欠落、エラーコード写像、ヘルス非計上を自動テストする
  - 完了条件: 該当テストが `cargo test` でパスする
  - _Depends: 2.1, 2.2, 2.3_
  - _Requirements: 5.2, 6.4, 7.1, 7.3, 8.1, 8.2, 8.3, 8.4_

- [ ] 5.2 起動非リッスン・解析成功／拒否・計上回数・過大ボディを結合検証する
  - 起動ゲート失敗でリッスンしないこと、成功時ヘルス 200 を確認する（`LicenseGate::install` 前提）
  - 解析成功、入力不正、推論拒否、ボディ過大、リクエストあたり計上 1 回をモック境界で検証する
  - HTTP ハンドラが `ensure_inference_allowed` を追加呼び出ししていないこと（二重計上なし）を固定する
  - 完了条件: 結合テストがパスし、二重計上および拒否時の結果漏洩がない
  - _Depends: 4.1_
  - _Requirements: 1.1, 1.3, 3.3, 4.1, 4.3, 4.4, 4.5, 5.1, 7.2, 8.2_

- [ ]* 5.3 実サーバー／埋め込みビルドでの手動スモーク手順を残す
  - 上流ライセンスサーバーと `release-embedded-http-api` 成果物が利用可能なときの確認観点（起動・1 解析・ヘルス）をテストコメントまたは example 注記に残す
  - 初期 CI 必須にはしない
  - 完了条件: 運用者が追えるスモーク観点がリポジトリ内に残っている
  - _Requirements: 2.2, 3.2, 9.1, 11.1_
