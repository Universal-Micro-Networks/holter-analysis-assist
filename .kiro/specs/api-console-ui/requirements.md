# Requirements Document

## Introduction

本仕様は、ホルター解析支援アプリケーション（Holter Analysis Assist）の HTTP 解析サービスに、検査会社担当者がブラウザから疎通確認と単発解析を行える簡易コンソール画面を追加する。画面は HTTP サービスと同一のプロセス／成果物から配信され、既存のヘルスおよび解析エンドポイントを呼び出すのみとし、解析契約・ライセンス計上・配布パッケージ本体の再設計は行わない。臨床最終判定 UI・波形エディタ・帳票・SPA 製品化・別フロントエンドサーバーは範囲外とする。

## Boundary Context

- **In scope**:
  - ブラウザで利用できる簡易コンソール画面（ECL アップロード、解析実行、結果の表示／ダウンロード、ヘルス確認）
  - HTTP 解析サービス同一プロセスからの画面配信（追加のフロントエンド専用サーバーを要しない）
  - 処理中表示および失敗時の利用者向けエラー表示
  - 画面文言の日本語化
  - 既存ヘルス／解析エンドポイントへのクライアント側呼び出し（契約の再定義なし）
  - 配布成果物への UI 同梱に関する受け渡し定義（本仕様がアセット／配信前提を定め、`packaging-distribution` が追従する）
- **Out of scope**:
  - 臨床最終判定 UI・波形エディタ・帳票・詳細波形ビューア
  - 別プロセス／別ホストのフロントエンドサーバー、および SPA フレームワークによる多ページ製品 UI
  - ライセンスサーバー・課金管理画面・複雑 IAM（OAuth 等）
  - 解析コア・モデル埋め込み・ライセンスゲート実装そのもの
  - Docker／インストーラ／Linux パッケージの本体設計（同梱パスの追記は隣接の Existing Spec Update）
  - `/health` および `/v1/analyze` のリクエスト／レスポンス契約の再発明
- **Adjacent expectations**:
  - 上流 `http-api`: `GET /health` および `POST /v1/analyze`（multipart フィールド `ecl`、任意 `format` 等）が利用可能であること。サイズ上限・タイムアウトは `[http]` 設定に従う
  - 上流 `license-client`／正本解析入口: 解析 1 回あたりの許可確認・利用計上は既存入口内で 1 回行われること。本仕様の UI／配信層は独自の追加計上を行わない
  - 下流 `packaging-distribution`: UI を含む同一 HTTP バイナリ成果物を配布対象とする旨を Existing Spec Update で追従すること（本仕様はパッケージ形式を所有しない）

## Requirements

### Requirement 1: 同一サービスからのコンソール画面配信

**Objective:** As an 検査会社の導入担当者, I want ブラウザで簡易コンソールを開ける, so that 追加のフロントエンドサーバーや専用クライアントなしに導入検証できる

#### Acceptance Criteria

1. When 利用者がコンソール画面の配信経路へブラウザでアクセスする, the Holter Analysis Assist Console UI shall 操作可能なコンソール画面を返す
2. The Holter Analysis Assist Console UI shall HTTP 解析サービスと同一プロセスから配信され、画面表示のために別プロセスのフロントエンド専用サーバーを必要としない
3. The Holter Analysis Assist Console UI shall コンソール画面の配信経路を、既存のヘルスおよび解析エンドポイントと区別可能な経路として提供する

### Requirement 2: ヘルス確認

**Objective:** As an 運用オペレータ, I want コンソールからヘルス状態を確認できる, so that curl なしにサービス疎通を検証できる

#### Acceptance Criteria

1. When 利用者がコンソール上でヘルス確認を実行する, the Holter Analysis Assist Console UI shall 既存のヘルスエンドポイントへ問い合わせ、成功／失敗が判別できる結果を画面に示す
2. The Holter Analysis Assist Console UI shall ヘルス確認のために独自のライセンス利用計上を行わない
3. If ヘルス問い合わせが失敗する（到達不可・非成功応答等）, the Holter Analysis Assist Console UI shall 利用者が失敗を認識できるメッセージを日本語で表示する

### Requirement 3: ECL アップロードと解析実行

**Objective:** As an 検査会社の担当者, I want ブラウザから ECL を選んで解析を実行できる, so that 単発の導入検証と簡易運用ができる

#### Acceptance Criteria

1. When 利用者がコンソールで ECL ファイルを選択し解析実行を指示する, the Holter Analysis Assist Console UI shall 既存の解析エンドポイントへ当該入力を送信する
2. The Holter Analysis Assist Console UI shall 解析リクエストの入出力を上流 `http-api` の契約に従い、解析エンドポイント契約を再定義しない
3. While 解析リクエストが処理中である, the Holter Analysis Assist Console UI shall 処理中であることが分かる表示を行う
4. If ファイルが未選択のまま解析実行が指示される, the Holter Analysis Assist Console UI shall 解析リクエストを送信せず、利用者に不足を示すメッセージを日本語で表示する

### Requirement 4: 解析結果の表示とダウンロード

**Objective:** As an 検査会社の担当者, I want 解析結果を画面で確認し必要なら保存できる, so that 疎通結果を手元に残せる

#### Acceptance Criteria

1. When 解析が成功する, the Holter Analysis Assist Console UI shall 応答内容を画面上で確認できる形で提示する
2. When 解析が成功する, the Holter Analysis Assist Console UI shall 応答内容を利用者がローカルへ保存できる手段を提供する
3. Where 解析応答が JSON 形式である, the Holter Analysis Assist Console UI shall 画面上で内容を確認できる
4. Where 解析応答が CSV 形式である, the Holter Analysis Assist Console UI shall 画面上での確認またはダウンロードのいずれか（あるいは両方）で利用者が結果を取得できる

### Requirement 5: 解析失敗時のエラー表示

**Objective:** As an 検査会社の担当者, I want 解析失敗の理由が画面で分かる, so that 設定不備や入力不備を切り分けできる

#### Acceptance Criteria

1. If 解析エンドポイントがクライアントエラーまたはサーバーエラーを返す, the Holter Analysis Assist Console UI shall 解析成功として扱わず、失敗であることが分かるメッセージを日本語で表示する
2. If 解析リクエストがタイムアウトまたは到達不可となる, the Holter Analysis Assist Console UI shall 失敗であることが分かるメッセージを日本語で表示する
3. The Holter Analysis Assist Console UI shall エラー表示において、上流が返す識別可能な失敗種別がある場合は利用者が区別できる情報を提示する（詳細の内部スタックは必須としない）

### Requirement 6: ライセンス二重計上の禁止

**Objective:** As a 製品責任者, I want コンソール経由の解析が既存計上経路のみを使う, so that UI 追加によって課金が二重にならない

#### Acceptance Criteria

1. When コンソールから解析を実行する, the Holter Analysis Assist Console UI shall 既存の解析エンドポイント呼び出しのみを行い、UI／画面配信層で独自の利用計上を追加しない
2. The Holter Analysis Assist Console UI shall ヘルス確認および静的画面配信のために利用計上を行わない
3. The Holter Analysis Assist Console UI shall 解析・ライセンス・HTTP API の契約を再発明せず、上流仕様のエンドポイントを消費する

### Requirement 7: 日本語 UI 文言

**Objective:** As an 国内の検査会社担当者, I want 画面の案内やボタン・エラーが日本語である, so that 導入検証を母語で進められる

#### Acceptance Criteria

1. The Holter Analysis Assist Console UI shall 主要な操作ラベル、案内文、処理中表示、エラーメッセージを日本語で提供する
2. When 利用者がコンソールを初めて開く, the Holter Analysis Assist Console UI shall 何をすればよいか分かる日本語の短い案内を表示する

### Requirement 8: サイズ上限・タイムアウトの上位準拠

**Objective:** As an 運用オペレータ, I want 大きな ECL や長時間解析の扱いが API 設定と一致する, so that UI だけが別ルールにならない

#### Acceptance Criteria

1. The Holter Analysis Assist Console UI shall ECL サイズ上限およびリクエストタイムアウトの制約を上流 `http-api` の設定に従う（UI 独自のより緩い上限を設けない）
2. If 上流のサイズ上限またはタイムアウトにより解析が拒否または失敗する, the Holter Analysis Assist Console UI shall その失敗を Requirement 5 に従い利用者へ示す

### Requirement 9: 配布への UI 同梱受け渡し

**Objective:** As a リリース担当者, I want コンソール UI が HTTP 成果物に含まれ配布へ引き継がれる, so that 導入先でも同一バイナリから画面を使える

#### Acceptance Criteria

1. The Holter Analysis Assist Console UI shall コンソール画面を HTTP 解析バイナリ成果物に含める前提を定義し、追加のフロントエンド配布物を必須としない
2. The Holter Analysis Assist Console UI shall 下流 `packaging-distribution` が Existing Spec Update で追従できるよう、UI が同一バイナリ配信である契約を明示する
3. The Holter Analysis Assist Console UI shall Windows および Linux 向けの同一成果物方針において、コンソール画面の利用可否が OS ごとに分岐しないこと（配信経路・操作は同等）

### Requirement 10: 範囲外機能の非提供

**Objective:** As a 製品責任者, I want 簡易コンソールの範囲を明確にする, so that 臨床ワークステーション化や SPA 化へスコープが膨張しない

#### Acceptance Criteria

1. The Holter Analysis Assist Console UI shall 臨床最終判定、波形編集、帳票出力を提供しない
2. The Holter Analysis Assist Console UI shall ライセンスサーバー管理や課金ダッシュボードを提供しない
3. The Holter Analysis Assist Console UI shall 利用者認証製品化（OAuth 等の複雑 IAM）を必須としない
