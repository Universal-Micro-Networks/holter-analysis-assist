# Requirements Document

## Introduction

検査会社向けホルター不整脈 AI（Holter Analysis Assist）の商用ライセンス運用のため、本製品は別プロジェクトのライセンスサーバーに対するクライアントとして、プロセス起動時の有効性確認と、1 推論ごとの許可確認・利用計上を行う。ライセンスサーバー本体・課金・管理 UI は本リポの範囲外とし、本仕様はクライアント振る舞いと設定項目に限定する。

**改訂（2026-09-29）:** ライセンスサーバー（別リポ `holter-analysis-assist-license-server`）のクライアント向け仕様が確定したため、合意済みの契約（起動時の有効性確認、推論 1 回分の利用記録、ライセンスキーによる認証、失敗区分）に追従する。あわせて、サーバー未整備の間に入れていた「到達できない場合は暫定で許可する」扱いを撤廃し、本来の fail-closed に戻す。

## Boundary Context

- **In scope**:
  - プロセス起動時のライセンス有効性確認（失敗時は起動拒否）
  - 1 推論ごとの許可確認と利用計上（失敗時はその推論を拒否）。正本入口は `analyze_ecl_with_source`。ラッパでの二重計上なし
  - プロセス全体の `LicenseGate` インストール（CLI / HTTP 同一契約。未 install は fail-closed）
  - 「1 推論」の定義（解析ジョブ単位。ONNX window 単位ではない）
  - ライセンスサーバー接続に必要な設定項目（少なくともサーバー URL。必要に応じ認証情報・タイムアウト等）を ini ファイルで指定できること。`config/license.ini.example` の `[license]` 正本
  - `src/analyze.rs` への推論ゲート挿入と `src/main.rs` への CLI 起動ゲート挿入（ModelSource 配線は model-embedding）
  - 失敗時の明確なエラー（起動失敗 / 推論拒否）
  - テスト時にライセンスサーバー実体へ依存しない検証手段
  - Windows / Linux で同一設定キーを用いること
  - ini に置く秘密情報の取り扱い（権限・ログマスク）の運用上の期待
  - 確定したライセンスサーバー契約への追従（起動時の有効性確認、推論直前の利用記録、ライセンスキー必須、失敗区分の識別、月間上限なしライセンスの扱い）
  - 暫定許可（サーバー到達不能・タイムアウト・非成功応答でも許可）の撤廃
- **Out of scope**:
  - ライセンスサーバー実装、課金・請求・管理画面
  - オフライン運用（サーバー到達必須）
  - ONNX window 単位での都度計上
  - モデル埋め込み、HTTP API のリソース設計、配布パッケージ
  - 当月の利用状況（使用回数・残り回数）の照会と画面表示（サーバー側には照会手段があるが、本改訂では利用しない）
  - 運営者向けの管理用 API（ライセンス発行・停止等。管理用トークンで認証する別系統）
- **Adjacent expectations**:
  - 上流: 別リポのライセンスサーバーが、合意した契約（到達先・認証・成功/失敗の意味）で応答すること。契約の正本はサーバー側リポのクライアント向けルート定義とする
  - 下流: CLI および将来の `http-api` が、同一のプロセス全体 `LicenseGate`（起動時 install）と、正本解析入口での推論時ゲートを通ること
  - 隣接 `model-embedding`: 正本公開入口 `analyze_ecl_with_source` と `ModelSource` を定義する。本仕様はゲート挿入を所有し、実装順は embedding → license
  - 隣接 `http-api`: `[http]` 設定とルートを所有。本仕様の `[license]` 正本と Gate 契約を消費し、ハンドラ側で追加 meter しない。本改訂で追加する拒否理由の区分（Requirement 11）を、API 呼出側が識別できる形で応答に反映する
  - 隣接 `inference-acceleration`: 比較サブコマンドは ECL あたり 2 回（基準と候補）正本入口を通るため、利用記録も 2 回となる（本改訂で変わらない）
  - 隣接 `packaging-distribution`: 配布時に `config/license.ini.example`（または同等）をコピー／参照するのみ。`[license]` キーを再定義しない

## Requirements

### Requirement 1: プロセス起動時のライセンス有効性確認

**Objective:** As an 運用オペレータ, I want プロセス起動時にライセンスが有効であることを確認される, so that 無効な環境では解析処理を開始できない

#### Acceptance Criteria

1. When プロセスが起動する, the Holter Analysis Assist shall ライセンスサーバーへ有効性確認を 1 回行う
2. When 起動時の有効性確認が成功する, the Holter Analysis Assist shall 通常の起動処理を続行する
3. If 起動時の有効性確認が失敗する（拒否・通信不能・タイムアウト・設定不備を含む）, the Holter Analysis Assist shall 起動を拒否し、解析処理を開始しない
4. The Holter Analysis Assist shall 起動時確認の失敗理由を、オペレータが起動失敗と識別できる形で提示する

### Requirement 2: 1 推論ごとの許可確認と利用計上

**Objective:** As a ライセンス管理者, I want 1 推論ごとに許可確認と利用計上が行われる, so that 商用利用回数を把握・制御できる

#### Acceptance Criteria

1. When 1 回の推論（Requirement 3 で定義）が開始される, the Holter Analysis Assist shall その推論に対する許可確認と利用計上を、推論実行前に 1 回行う
2. When 推論時の許可確認と利用計上が成功する, the Holter Analysis Assist shall 当該推論の実行を許可する
3. If 推論時の許可確認または利用計上が失敗する（拒否・通信不能・タイムアウト・設定不備を含む）, the Holter Analysis Assist shall 当該推論を拒否し、解析結果を出力しない
4. The Holter Analysis Assist shall 推論拒否の理由を、オペレータまたは呼出側が推論拒否と識別できる形で提示する
5. The Holter Analysis Assist shall 同一推論内の複数 ONNX window 処理に対して、追加の許可確認・利用計上を行わない
6. If 入力（ECL のファイル名・内容）の不備、またはモデルの読み込み失敗により推論に進めない, the Holter Analysis Assist shall 利用計上を行わずにその失敗を返す

### Requirement 3: 「1 推論」の定義

**Objective:** As a ライセンス管理者, I want 「1 推論」が解析ジョブ単位で定義される, so that fullday 解析で数千回の window 課金が起きない

#### Acceptance Criteria

1. The Holter Analysis Assist shall 「1 推論」を、正本公開入口 `analyze_ecl_with_source` が実行する 1 解析ジョブ（CLI の `analyze-ecl` 相当、または API の 1 解析リクエスト）として扱う
2. The Holter Analysis Assist shall ONNX の 1 window を「1 推論」として扱わない
3. When 1 解析ジョブまたは 1 解析リクエストが複数 window の推論を含む, the Holter Analysis Assist shall それら全体を 1 推論として許可確認・利用計上する
4. The Holter Analysis Assist shall 互換ラッパ（`analyze_ecl` / `analyze_ecl_with_limit`）経由でも追加の許可確認・利用計上を行わず、正本入口での 1 回に限定する

### Requirement 4: ini による接続設定

**Objective:** As an 運用オペレータ, I want ライセンスサーバー接続情報を ini ファイルで設定できる, so that 環境ごとにエンドポイント等を切り替えられる

#### Acceptance Criteria

1. The Holter Analysis Assist shall ライセンスサーバーの到達先（URL）を ini ファイルから読み取る
2. Where 認証情報またはタイムアウト等の追加接続パラメータが必要な場合, the Holter Analysis Assist shall それらを同一の ini 設定体系で指定できる
3. The Holter Analysis Assist shall Windows と Linux で同一の設定キー名を用いる
4. If 必須の接続設定が欠落している、または不正である, the Holter Analysis Assist shall 起動時確認または推論時確認を失敗扱いとし、fail-closed で拒否する
5. The Holter Analysis Assist shall 設定キーの意味を、オペレータが環境差分なく解釈できる形で文書化する
6. The Holter Analysis Assist shall `[license]` セクションの正本サンプルを `config/license.ini.example`（または合意した同一パス）として提供し、他機能がキー意味を再定義しない前提を文書化する

### Requirement 5: 失敗時の明確なエラー区分

**Objective:** As an 運用オペレータ, I want 起動失敗と推論拒否を区別できる, so that 障害対応とライセンス切れを切り分けられる

#### Acceptance Criteria

1. When 起動時の有効性確認が失敗する, the Holter Analysis Assist shall 起動失敗として識別可能なエラーを返す
2. When 推論時の許可確認または利用計上が失敗する, the Holter Analysis Assist shall 推論拒否として識別可能なエラーを返す
3. The Holter Analysis Assist shall 失敗メッセージに、少なくとも失敗区分（起動失敗 / 推論拒否）と、オペレータが次の行動を判断できる概要を含める

### Requirement 6: テスト可能なモック境界

**Objective:** As a 開発者, I want ライセンスサーバー実体なしでクライアント振る舞いを検証できる, so that CI と単体テストで fail-closed を確認できる

#### Acceptance Criteria

1. The Holter Analysis Assist shall ライセンスサーバーへの実通信を差し替え可能な境界を提供する
2. When テストがモック境界を用いる, the Holter Analysis Assist shall 外部ライセンスサーバーへ到達せずに、成功・拒否・通信失敗などの応答パターン（Requirement 11 の各拒否理由と、月間上限なしの成功を含む）を再現できる
3. The Holter Analysis Assist shall 起動時確認と推論時確認の両方について、モック境界経由の自動テストで検証可能である

### Requirement 7: 秘密情報の取り扱い

**Objective:** As an 運用オペレータ, I want ini 上の秘密情報が不用意に露出しない, so that API キー等の漏洩リスクを低減できる

#### Acceptance Criteria

1. Where ini に認証情報などの秘密情報を置く場合, the Holter Analysis Assist shall ログおよび通常のエラー出力にその秘密情報の平文を含めない
2. The Holter Analysis Assist shall 秘密情報を含む設定ファイルについて、OS のファイル権限で保護することを運用上の前提として文書化する
3. If 秘密情報をログやエラーに出力しそうな状況が発生する, the Holter Analysis Assist shall マスクまたは省略した表現を用いる

### Requirement 8: オンライン必須と範囲外の明示

**Objective:** As a ステークホルダー, I want 初期スコープの境界が明確である, so that オフライン運用やサーバー実装を本仕様に期待しない

#### Acceptance Criteria

1. The Holter Analysis Assist shall 初期対象として、ライセンスサーバーへ到達できない環境での解析継続（オフライン運用）を提供しない
2. The Holter Analysis Assist shall ライセンスサーバー実装、課金・請求、管理画面を本機能の成果として提供しない
3. The Holter Analysis Assist shall モデル埋め込み、HTTP API のリソース設計、配布パッケージの所有権を本機能に含めない（ただし共有ファイルへのゲート挿入、および `[license]` 正本の提供は本機能の範囲内）
4. If ライセンスサーバーへ到達できない、時間内に応答しない、または成功以外を応答する, the Holter Analysis Assist shall 暫定的な許可を行わず、起動または推論を拒否する
5. The Holter Analysis Assist shall 当月の利用状況（使用回数・残り回数）の照会と表示、および運営者向け管理用 API の利用を本改訂の成果として提供しない

### Requirement 9: プロセス全体ゲートと fail-closed 注入

**Objective:** As a 開発者, I want ライセンスゲートがプロセス起動時に一度だけインストールされ解析入口から参照される, so that CLI と HTTP が同一計上点を共有し ModelSource シグネチャを壊さない

#### Acceptance Criteria

1. When CLI または HTTP プロセスが起動する, the Holter Analysis Assist shall `LicenseGate` を構築しプロセス全体にインストールしたうえで起動時有効性確認を行う
2. When 正本公開入口 `analyze_ecl_with_source` が推論を開始する, the Holter Analysis Assist shall インストール済みゲートを `global` / `try_global` 相当で取得し、許可確認・利用計上を行う（解析関数への Gate 引数追加を v1 では行わない）
3. If 計上が必要な経路でゲートが未インストールである, the Holter Analysis Assist shall fail-closed で当該推論を拒否する
4. The Holter Analysis Assist shall HTTP 起動経路も CLI と同一のインストール契約を用いることを、隣接仕様向け契約として文書化する

### Requirement 10: 確定したライセンスサーバー契約への追従

**Objective:** As a ライセンス管理者, I want クライアントが確定したライセンスサーバー契約どおりに有効性確認と利用記録を行う, so that サーバー側の利用回数集計と上限制御がそのまま機能する

#### Acceptance Criteria

1. When プロセスが起動する, the Holter Analysis Assist shall ライセンスサーバーの有効性確認を呼び出し、サーバーがライセンスを有効と応答した場合にのみ起動を続行する
2. When 1 推論が開始される, the Holter Analysis Assist shall 推論の直前にライセンスサーバーへ推論 1 回分の利用記録を要求し、サーバーが利用記録の成功を応答した場合にのみ当該推論を実行する
3. If ライセンスサーバーが利用記録の成功以外を応答する, the Holter Analysis Assist shall 当該推論を実行しない
4. The Holter Analysis Assist shall ライセンスサーバーへの要求に、推論の入力（心電図データ）と解析結果を含めない
5. The Holter Analysis Assist shall ライセンスキーを要求の認証情報としてのみ送信し、要求先の URL や要求本文に含めない
6. If ライセンスサーバーの応答が合意した形式として解釈できない, the Holter Analysis Assist shall 当該の起動または推論を拒否する
7. The Holter Analysis Assist shall 利用の日時と月の区切りをライセンスサーバー側の判定に委ね、クライアント側の時計で許可・拒否を判定しない

### Requirement 11: 拒否理由の区分

**Objective:** As an 運用オペレータ, I want ライセンスによる拒否の理由を区別できる, so that キーの設定誤り・利用停止・上限到達・一時障害のどれかを判断して次の行動を取れる

#### Acceptance Criteria

1. When ライセンスサーバーがキーの欠落または形式不正を応答する, the Holter Analysis Assist shall 拒否理由を「キーの設定不備」として識別できる形で提示する
2. When ライセンスサーバーが未登録のキーであると応答する, the Holter Analysis Assist shall 拒否理由を「ライセンス無効」として識別できる形で提示する
3. When ライセンスサーバーが利用停止中であると応答する, the Holter Analysis Assist shall 拒否理由を「利用停止」として識別できる形で提示する
4. When ライセンスサーバーが当月の上限到達を応答する, the Holter Analysis Assist shall 当該推論を拒否し、拒否理由を「当月上限到達」として識別できる形で提示する
5. When ライセンスサーバーが要求過多を応答する, the Holter Analysis Assist shall 当該の起動または推論を拒否し、拒否理由を「要求過多（時間をおいて再試行）」として識別できる形で提示する
6. If ライセンスサーバーが一時的な障害を応答する、到達できない、または設定した時間内に応答しない, the Holter Analysis Assist shall 当該の起動または推論を拒否し、拒否理由を「一時障害（時間をおいて再試行）」として識別できる形で提示する
7. Where 推論が HTTP API 経由で要求される場合, the Holter Analysis Assist shall API 呼出側が、時間をおいて再試行できる拒否（要求過多・一時障害）と、それ以外の拒否を区別できる応答を返す
8. The Holter Analysis Assist shall 拒否理由の提示に、ライセンスサーバーが返した説明文を秘密情報を含まない範囲で含める

### Requirement 12: ライセンスキー設定と月間上限なしライセンス

**Objective:** As an 運用オペレータ, I want ライセンスキーの設定方法が明確で、上限なしライセンスが誤って拒否されない, so that 発行されたライセンスをそのまま運用できる

#### Acceptance Criteria

1. The Holter Analysis Assist shall ライセンスキーを `[license]` セクションの `license_key` から読み取る
2. If `license_key` が未設定または空である, the Holter Analysis Assist shall 起動時確認を失敗扱いとし、キーが未設定であることを提示して起動を拒否する
3. If `[license]` セクションに旧設定名 `api_key` だけが書かれている, the Holter Analysis Assist shall `api_key` をライセンスキーとして使わず、`license_key` への書き換えが必要であることを提示して起動を拒否する
4. When ライセンスサーバーが月間上限 0（上限なし）のライセンスを有効と応答する, the Holter Analysis Assist shall そのライセンスを上限なしとして扱い、上限 0 を理由に起動または推論を拒否しない
5. When 上限なしライセンスで利用記録が成功する, the Holter Analysis Assist shall 残り回数が示されないことを理由に当該推論を拒否しない

### Requirement 13: ライセンス確認結果の記録

**Objective:** As an 運用オペレータ, I want ライセンスサーバーとのやり取りが成功したこともログで確認できる, so that ライセンスサーバーにつながって計上されているかを、失敗が起きる前に判断できる

> **改訂メモ（2026-09-29）**: 初版では成功時に何も出力しなかったため、ライセンスサーバーにつながっているのかをログから判断できなかった（「つながらなくても実行できたように見える」との指摘）。fail-closed の挙動（1.3・2.3・8.4）は変わらず、成功時の記録だけを追加した。ここで出す使用回数は、利用記録の成功応答にすでに含まれる値をそのまま書くもので、利用状況の照会（8.5）ではない。画面への表示も行わない。

#### Acceptance Criteria

1. When 起動時の有効性確認が成功する, the Holter Analysis Assist shall 確認が成功したことを、ライセンスの状態と月間上限（上限なしは上限なしと分かる表記）を添えて標準エラーに 1 行出力する
2. When 推論時の利用記録が成功する, the Holter Analysis Assist shall 利用記録が成功したことを、成功応答に含まれる使用回数・月間上限・残り回数（上限なしは上限なしと分かる表記）を添えて標準エラーに 1 行出力する
3. The Holter Analysis Assist shall 成功時の記録に、ライセンスキーの平文を含めない
4. The Holter Analysis Assist shall 成功時の記録のために、ライセンスサーバーへの追加の要求（利用状況の照会など）を行わない
