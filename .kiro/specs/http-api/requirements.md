# Requirements Document

## Introduction

本仕様は、ホルター解析支援アプリケーション（Holter Analysis Assist）に HTTP による解析面を追加し、クラウド連携や他システムから ECL 入力に対する解析を呼べるようにする。解析品質・埋め込みモデル利用・ライセンスゲートは CLI と同一コアを共有し、起動時は `LicenseGate::install` と有効性確認に失敗した場合はサーバーを起動しない。各解析リクエストでは正本入口 `analyze_ecl_with_source` 内で許可確認・利用計上を 1 回行う（HTTP は meter を直接呼ばない。ONNX window 単位ではない）。ライセンスサーバー本体、配布パッケージ本体、本格的な利用者認証（IAM / OAuth）は本仕様の範囲外とする。

## Boundary Context

- **In scope**:
  - ECL（または合意した解析入力）を受け取り、解析結果（少なくとも CLI と同等の CSV 相当、および機械可読な JSON 要約または行データ）を返す解析用 HTTP エンドポイント
  - ヘルスチェック等の最小運用エンドポイント
  - プロセス起動時のライセンス有効性確認（失敗時は HTTP サービスをリッスンしない）
  - 各解析リクエストあたり 1 回のライセンス許可確認・利用計上（正本入口内。HTTP は meter を直接呼ばない。失敗時は解析結果を返さない）
  - 埋め込みモデル経路を用いた推論（配布想定）と、開発用途でのパス指定モデルの利用方針
  - リッスンアドレス等の HTTP 設定（`[http]`）およびライセンス接続設定を ini で読み取ること（`[license]` キー名は `license-client` 正本と同一。再定義しない）
  - 大きな ECL アップロードに対するサイズ上限と処理タイムアウトの明示
  - Windows（ローカル常駐）および Linux（クラウドプロセス）での同一バイナリ用途
  - CI ジョブ `release-embedded-http-api` による埋め込み `holter-http-api` 成果物（packaging 入力）
- **Out of scope**:
  - ライセンスサーバー実装・課金 DB・管理 UI
  - Docker / Windows インストーラ / Linux パッケージの作成本体（呼び出し方の文書化は可）
  - 複雑なマルチテナント IAM / OAuth 製品化
  - モデル埋め込み実装そのもの（`model-embedding` の成果を消費するのみ）
  - ライセンスゲート実装そのもの（`license-client` の成果を消費するのみ）
- **Adjacent expectations**:
  - 上流 `model-embedding`: `ModelSource` および正本公開入口 `analyze_ecl_with_source` が利用可能であること。本仕様は解析リクエストで当該正本入口のみを呼び、並列の gated 入口を発明しない
  - 上流 `license-client`: process-wide `LicenseGate::install` / `ensure_startup_licensed`、および正本入口内の推論ゲート。`[license]` キー正本は `config/license.ini.example`。API の 1 解析リクエストは「1 推論」として正本入口内で計上されること。HTTP は `ensure_inference_allowed` / meter を直接呼ばない
  - 隣接 CLI: 入出力契約の二重管理を避け、同一 lib 解析パイプラインを呼ぶこと
  - 下流 `packaging-distribution`: 本仕様の CI 成果物 `release-embedded-http-api`（埋め込み `holter-http-api`）を配布対象としうるが、本仕様はパッケージ形式を所有しない

## Requirements

### Requirement 1: 解析 HTTP エンドポイント

**Objective:** As a 連携システム運用者, I want ECL を HTTP で送信して解析結果を受け取れる, so that CLI なしで他システムから同一品質の解析を利用できる

#### Acceptance Criteria

1. When 利用者が有効な ECL 入力を解析エンドポイントへ送信する, the Holter Analysis Assist HTTP Service shall CLI の `analyze-ecl` 相当と同一コアの解析を実行し、解析結果を返す
2. When 解析が成功する, the Holter Analysis Assist HTTP Service shall 少なくとも beat 単位結果を CSV 相当で取得できる手段、および機械可読な JSON（要約または行データのいずれか、あるいは両方）を提供する
3. If 入力が不正である（形式不正・必須不足・空入力等）, the Holter Analysis Assist HTTP Service shall 解析を実行せず、クライアントエラーとして識別可能な応答を返す
4. The Holter Analysis Assist HTTP Service shall 解析結果のラベル体系（リズム分類等）を CLI と同等に保つ

### Requirement 2: CLI と同一コアの共有

**Objective:** As a 製品責任者, I want HTTP 面が CLI と同一の解析・モデル・ライセンス方針を共有する, so that 面ごとに契約や品質が分岐しない

#### Acceptance Criteria

1. The Holter Analysis Assist HTTP Service shall 解析ビジネスロジックを HTTP アダプタ層に重複実装せず、正本公開入口 `analyze_ecl_with_source` を呼び出す（並列の gated 入口を設けない）
2. When 埋め込みモデルを含む配布想定バイナリで解析リクエストを処理する, the Holter Analysis Assist HTTP Service shall 外部の生モデルファイルを必須とせずに推論できる
3. Where 開発用のパス指定モデルロードが有効である, the Holter Analysis Assist HTTP Service shall 設定または同等の明示手段でパス指定モデルを利用できる
4. The Holter Analysis Assist HTTP Service shall ライセンスゲートおよびモデルソース契約を、上流仕様で定義された公開契約と矛盾する形で再定義しない

### Requirement 3: 起動時ライセンス確認とサーバー起動拒否

**Objective:** As an 運用オペレータ, I want 起動時にライセンスが無効なら HTTP サービスが立たない, so that 無許可環境でリッスン面が露出しない

#### Acceptance Criteria

1. When HTTP サービスプロセスが起動する, the Holter Analysis Assist HTTP Service shall 上流 `LicenseGate` を process-wide に `install` し、有効性確認（`ensure_startup_licensed`）を 1 回行う
2. When 起動時の有効性確認が成功する, the Holter Analysis Assist HTTP Service shall 設定されたアドレスでリクエスト受付を開始する
3. If 起動時の有効性確認が失敗する（拒否・通信不能・タイムアウト・設定不備を含む）, the Holter Analysis Assist HTTP Service shall HTTP ポートのリッスンを開始せず、起動失敗として識別可能な形で終了する
4. The Holter Analysis Assist HTTP Service shall 起動失敗の理由を、オペレータが起動失敗と識別できる形で提示する

### Requirement 4: 解析リクエストごとのライセンス確認・計上

**Objective:** As a ライセンス管理者, I want 各解析リクエストで 1 回だけ許可確認と利用計上が行われる, so that API 利用が CLI ジョブと同一の計上単位になる

#### Acceptance Criteria

1. When 1 件の解析リクエストの処理を開始する, the Holter Analysis Assist HTTP Service shall 正本入口 `analyze_ecl_with_source` を呼び出すことで、許可確認と利用計上が解析実行前に 1 回行われるようにする（HTTP 層自身は `ensure_inference_allowed` / meter を呼ばない）
2. When 当該リクエストの許可確認と利用計上が成功する, the Holter Analysis Assist HTTP Service shall 解析の実行結果を返せる
3. If 当該リクエストの許可確認または利用計上が失敗する, the Holter Analysis Assist HTTP Service shall 解析結果を返さず、推論拒否として識別可能な応答を返す
4. The Holter Analysis Assist HTTP Service shall 同一解析リクエスト内の複数 ONNX window 処理に対して、追加の許可確認・利用計上を行わない
5. The Holter Analysis Assist HTTP Service shall 「1 推論」を API の 1 解析リクエストとして扱い、CLI の `analyze-ecl` 相当ジョブと同一定義を用いる

### Requirement 5: ヘルスチェック等の最小運用面

**Objective:** As an 運用オペレータ, I want プロセス生存を確認できる, so that ロードバランサや監視から到達可否を判定できる

#### Acceptance Criteria

1. When 利用者がヘルスチェック用エンドポイントへ問い合わせる, the Holter Analysis Assist HTTP Service shall プロセスがリクエストを受け付け可能な状態であることを示す成功応答を返す
2. The Holter Analysis Assist HTTP Service shall ヘルスチェック応答のために利用計上（meter）を行わない
3. The Holter Analysis Assist HTTP Service shall ヘルスチェックを、解析エンドポイントとは区別可能な経路として提供する

### Requirement 6: ini による HTTP およびライセンス設定

**Objective:** As an 運用オペレータ, I want リッスン設定とライセンス接続を ini で切り替えられる, so that Windows ローカルと Linux クラウドで同一キー体系を使える

#### Acceptance Criteria

1. The Holter Analysis Assist HTTP Service shall リッスンアドレス（ホストおよびポート、または同等の bind 指定）を `[http]` セクションから読み取る（`[http]` キー意味は本仕様が所有）
2. The Holter Analysis Assist HTTP Service shall ライセンス接続設定を、上流 `license-client` の正本（`config/license.ini.example` の `[license]`）と同一のセクションおよびキー名で読み取る（キーを再定義しない）
3. The Holter Analysis Assist HTTP Service shall Windows と Linux で同一の設定キー名を用いる
4. If 必須の HTTP またはライセンス設定が欠落している、または不正である, the Holter Analysis Assist HTTP Service shall 起動を拒否する（fail-closed）
5. The Holter Analysis Assist HTTP Service shall `[http]` の設定キー意味を文書化し、`[license]` については上流正本への参照または同一ファイルへのマージ手順を示す（`config/http.ini.example` または `license.ini.example` への `[http]` 追記）

### Requirement 7: アップロードサイズ上限と処理タイムアウト

**Objective:** As an 運用オペレータ, I want 大きな ECL に対する上限が明示されている, so that 過大入力や長時間ハングを運用上制御できる

#### Acceptance Criteria

1. The Holter Analysis Assist HTTP Service shall 解析リクエスト本体（ECL ペイロード）の既定最大サイズを 512 MiB とする
2. If 解析リクエスト本体が既定または設定された最大サイズを超える, the Holter Analysis Assist HTTP Service shall 解析を開始せず、クライアントエラーとして識別可能な応答を返す
3. The Holter Analysis Assist HTTP Service shall 1 解析リクエストの端到端処理について、既定の最大処理時間を 1800 秒（30 分）とする
4. If 1 解析リクエストの処理が既定または設定された最大処理時間を超える, the Holter Analysis Assist HTTP Service shall 当該リクエストを失敗として扱い、クライアントがタイムアウト／処理失敗と識別できる応答または切断を行う
5. Where サイズ上限または処理タイムアウトを運用で変更する必要がある場合, the Holter Analysis Assist HTTP Service shall それらを ini で上書き指定できる

### Requirement 8: 失敗応答の区分

**Objective:** As a 連携システム開発者, I want 失敗種別を区別できる, so that 再試行・設定修正・ライセンス対応を切り分けられる

#### Acceptance Criteria

1. When 入力不正により解析できない, the Holter Analysis Assist HTTP Service shall クライアント起因の入力エラーとして識別可能な応答を返す
2. When ライセンス推論拒否により解析できない, the Holter Analysis Assist HTTP Service shall 推論拒否として識別可能な応答を返す
3. When サーバー内部エラーにより解析できない, the Holter Analysis Assist HTTP Service shall サーバーエラーとして識別可能な応答を返す
4. The Holter Analysis Assist HTTP Service shall 失敗応答に、少なくとも失敗区分と、呼出側が次の行動を判断できる概要を含める（秘密情報の平文は含めない）

### Requirement 9: 対象プラットフォームと library-first

**Objective:** As a リリース担当者, I want 同一 HTTP サービスを Windows と Linux で運用できる, so that ローカル常駐とクラウドプロセスの両方に展開できる

#### Acceptance Criteria

1. The Holter Analysis Assist HTTP Service shall Windows（x86_64）および Linux（x86_64）で同一の設定キーおよびエンドポイント契約で運用可能である
2. The Holter Analysis Assist HTTP Service shall HTTP アダプタを薄く保ち、分類・前処理・推論・後処理のビジネスロジックをライブラリ側に置く
3. The Holter Analysis Assist HTTP Service shall CLI バイナリと相互依存せず、いずれもライブラリに依存する構成を維持する

### Requirement 10: 範囲外の明確化

**Objective:** As a ステークホルダー, I want 本仕様が担わない責務が明確である, so that ライセンスサーバー・配布・IAM を本仕様に期待しない

#### Acceptance Criteria

1. The Holter Analysis Assist HTTP Service shall ライセンスサーバー実装、課金・請求、管理画面を本機能の成果として提供しない
2. The Holter Analysis Assist HTTP Service shall Docker イメージ、Windows インストーラ、Linux パッケージの作成本体を本機能の成果として提供しない
3. The Holter Analysis Assist HTTP Service shall マルチテナント向けの本格 IAM / OAuth 製品機能を初期範囲に含めない
4. The Holter Analysis Assist HTTP Service shall モデル埋め込み実装およびライセンスゲート実装の所有権を本機能に含めず、上流仕様の公開契約を消費する

### Requirement 11: 埋め込み HTTP 成果物（CI）

**Objective:** As a リリース担当者, I want 埋め込みモデル付き `holter-http-api` が CI から得られる, so that packaging-distribution が配布入力を消費できる

#### Acceptance Criteria

1. The Holter Analysis Assist HTTP Service shall CI ジョブ名 `release-embedded-http-api` で、`embedded-model` feature 付きの `holter-http-api` バイナリ成果物を生成する
2. The Holter Analysis Assist HTTP Service shall 当該成果物に生モデルファイルを含めず、下流 `packaging-distribution` が消費可能な形で公開する
3. The Holter Analysis Assist HTTP Service shall CLI 用ジョブ `release-embedded-cli` を所有せず、その契約を変更しない
