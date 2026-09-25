# Requirements Document

## Introduction

本仕様は、ホルター解析支援アプリケーション（Holter Analysis Assist）の HTTP サービスおよび関連バイナリを、検査会社のローカル環境と Linux クラウドへ容易に展開できる配布成果物として整備する。運用者はコンテナイメージ、Windows 向けインストーラ、Linux 向けパッケージのいずれかで導入でき、生モデルファイルを成果物から分離した埋め込みバイナリ前提の配布を受け取る。サンプル設定（ini）と第三者ライセンス告知（NOTICE）を同梱し、インストール後にライセンス接続先を設定してサービスを起動できる。ライセンスサーバー本体、API 契約の再設計、クラウド IaC、高度な自動更新チャネルは本仕様の範囲外とする。

## Boundary Context

- **In scope**:
  - Linux クラウド向けコンテナイメージの作成・公開と実行手順の文書化
  - Windows（x86_64）向けインストーラ成果物と導入・起動手順の文書化
  - Linux（x86_64）向け配布パッケージ（少なくとも 1 形式。実行可能な場合は deb と rpm の両方）
  - 配布成果物いずれにも生モデルファイルを含めないこと（埋め込みバイナリ前提）
  - CPU 既定バリアントを必須とし、CUDA 付きは任意の別成果物として分離できること
  - ライセンス接続および HTTP 起動に必要なサンプル ini の同梱
  - ORT 等の第三者ライセンスを示す NOTICE の同梱
  - CI からの成果物組み立て・公開手段
  - サービス起動方法および導入後の設定手順の文書化
- **Out of scope**:
  - ライセンスサーバー本体・課金 DB・管理 UI の配布または実装
  - HTTP API のエンドポイント契約・エラー区分・ini キー意味の再設計（`[license]` / `[http]` 正本の所有を含む）
  - モデル埋め込みロジックそのもの（上流 `model-embedding` の成果を消費するのみ）
  - model-embedding の CI ジョブ `release-embedded-cli` の定義・再定義
  - クラウド IaC（Terraform 等）の本格整備
  - 自動更新チャネルの高度化（サイレント更新チャネル設計等）
  - ARM 等 x86_64 以外のターゲット
- **Adjacent expectations**:
  - 上流 `http-api`: 埋め込みモデルを含むリリース用 HTTP バイナリを artifact `release-embedded-http-api`（埋め込み `holter-http-api`）として提供すること。`[http]` サンプル example を提供すること
  - 上流 `api-console-ui`（Existing Spec Update）: 簡易コンソール UI は同一 `holter-http-api` バイナリに埋め込み配信され、追加のフロントエンド成果物を必須としない。正本 URL は `/ui/`（Windows／Linux 同一）
  - 上流 `model-embedding`: リリース成果物に生モデルを含めない埋め込み方針が適用済みであること。本仕様はジョブ `release-embedded-cli` を消費・再定義しない
  - 上流 `license-client`: `[license]` 正本 `config/license.ini.example` が利用可能であること。本仕様はコピー／参照（またはパッケージ時マージ）のみでキー意味を変更しない
  - roadmap の直接依存は `http-api` のみだが、ini・埋め込み非同梱契約により `license-client` / `model-embedding` に **推移的に** 依存する。コンソール UI の導入文書追従は `api-console-ui` 完了後
  - 既存 CI: `release-embedded-http-api` を入力として配布成果物を追加できること

## Requirements

### Requirement 1: Linux クラウド向けコンテナイメージ

**Objective:** As a クラウド運用オペレータ, I want Holter Analysis Assist をコンテナイメージとして取得・起動できる, so that Linux クラウドへ手作業バイナリ配置なしで展開できる

#### Acceptance Criteria

1. When リリース担当者が Linux 向け配布ビルドを実行する, the Packaging Distribution System shall Linux クラウドで利用可能なコンテナイメージ成果物を生成する
2. When 運用オペレータが文書化された手順に従いコンテナイメージを起動する, the Packaging Distribution System shall HTTP 解析サービスとしてリクエスト受付可能な状態へ到達できる手段を提供する
3. The Packaging Distribution System shall コンテナイメージの取得方法・起動方法・必須設定（少なくともライセンス接続とリッスン）を運用者が追える形で文書化する
4. The Packaging Distribution System shall コンテナイメージ成果物の対象アーキテクチャを Linux x86_64 とする

### Requirement 2: Windows 向けインストーラ

**Objective:** As a Windows ローカル運用オペレータ, I want インストーラで導入できる, so that 手動コピー手順なしでローカル常駐サービスを設置できる

#### Acceptance Criteria

1. When リリース担当者が Windows 向け配布ビルドを実行する, the Packaging Distribution System shall Windows x86_64 向けインストーラ成果物を生成する
2. When 運用オペレータがインストーラを実行する, the Packaging Distribution System shall HTTP 解析サービス実行に必要なバイナリおよび同梱ファイルを導入先へ配置できる
3. The Packaging Distribution System shall インストール後の設定（少なくともサンプル ini の配置または参照方法）と起動手順を文書化する
4. If インストーラ実行が失敗する, the Packaging Distribution System shall 運用者が失敗と識別できる結果を提示する（サイレント失敗にしない）

### Requirement 3: Linux 向け配布パッケージ

**Objective:** As a Linux 運用オペレータ, I want deb または rpm 等のパッケージで導入できる, so that コンテナ以外の Linux ホストでも標準的なパッケージ管理で展開できる

#### Acceptance Criteria

1. When リリース担当者が Linux 向け配布ビルドを実行する, the Packaging Distribution System shall Linux x86_64 向けの配布パッケージを少なくとも 1 形式生成する
2. Where deb および rpm の両方の生成が実行可能である, the Packaging Distribution System shall 両形式の成果物を提供する
3. When 運用オペレータが提供されたパッケージ形式でインストールする, the Packaging Distribution System shall HTTP 解析サービス実行に必要なバイナリおよび同梱ファイルを導入先へ配置できる
4. The Packaging Distribution System shall パッケージ導入後の設定と起動手順を文書化する

### Requirement 4: 生モデルファイルの非同梱

**Objective:** As a 製品配布担当者, I want いずれの配布成果物にも生モデルファイルが含まれない, so that モデル資産のカジュアルな持ち出しを抑えられる

#### Acceptance Criteria

1. When コンテナイメージ・Windows インストーラ・Linux パッケージいずれかの成果物を組み立てる, the Packaging Distribution System shall 生モデルファイル（例: `.onnx` や同等の生重みファイル）を同梱しない
2. The Packaging Distribution System shall 配布成果物が上流の埋め込みモデル付きリリースバイナリを前提とすることを明示する
3. If 配布ビルド入力として生モデルファイルの同梱が検出される, the Packaging Distribution System shall 当該成果物を合格扱いにせず、検出可能な失敗または検証失敗とする

### Requirement 5: CPU 既定と CUDA 任意分離

**Objective:** As a リリース担当者, I want CPU 既定の配布を必須とし CUDA 付きは別成果物にできる, so that GPU 非必須環境でも標準導入でき、GPU 需要は任意に分離できる

#### Acceptance Criteria

1. The Packaging Distribution System shall CPU 実行を既定とする配布バリアントを必須成果物として提供する
2. Where CUDA 付きバリアントを提供する場合, the Packaging Distribution System shall それを CPU 既定成果物とは別の成果物として分離する
3. The Packaging Distribution System shall CUDA 付きバリアントの提供を必須としない
4. The Packaging Distribution System shall どの成果物が CPU 既定か（および任意の CUDA 付きか）を運用者が識別できる形で文書化する

### Requirement 6: サンプル ini の同梱

**Objective:** As an 運用オペレータ, I want ライセンス接続と HTTP 起動用のサンプル ini が同梱される, so that インストール後すぐに接続先を設定して起動できる

#### Acceptance Criteria

1. When 配布成果物（コンテナイメージ・Windows インストーラ・Linux パッケージ）を組み立てる, the Packaging Distribution System shall ライセンス接続設定を含むサンプル ini を同梱またはイメージ内に配置する
2. The Packaging Distribution System shall サンプル ini のライセンス関連キーを上流 `license-client` の `config/license.ini.example` からコピーまたは参照して提供する（少なくとも到達先 URL）。キー正本を本仕様で所有・再定義しない
3. The Packaging Distribution System shall サンプル ini の HTTP 関連キーを上流 `http-api` の `[http]` example からコピーまたは参照して提供する（少なくともリッスン指定）。必要ならパッケージ時に両ソースを単一ランタイム用サンプルへマージしてよい
4. The Packaging Distribution System shall サンプル ini のキー意味および設置場所を文書化する
5. The Packaging Distribution System shall ライセンスサーバー URL 等のキー意味を本仕様で再定義しない

### Requirement 7: 第三者 NOTICE の同梱

**Objective:** As a コンプライアンス担当者, I want ORT 等の第三者ライセンス告知が成果物に含まれる, so that 再配布時の通知義務を満たせる

#### Acceptance Criteria

1. When 配布成果物を組み立てる, the Packaging Distribution System shall 第三者コンポーネント（少なくとも ORT 関連）のライセンス告知を含む NOTICE を同梱またはイメージ内に配置する
2. The Packaging Distribution System shall NOTICE の所在を運用者または導入担当者が参照できる形で文書化する
3. If NOTICE が欠落した成果物組み立てが検出される, the Packaging Distribution System shall 当該成果物を合格扱いにせず、検出可能な失敗または検証失敗とする

### Requirement 8: CI からの成果物公開

**Objective:** As a リリース担当者, I want CI から配布成果物を組み立てて公開できる, so that 手作業ビルドに依存せず再現可能な配布ができる

#### Acceptance Criteria

1. When リリース用 CI が成功する, the Packaging Distribution System shall 本仕様の必須配布成果物（CPU 既定のコンテナイメージ、Windows インストーラ、Linux パッケージ少なくとも 1 形式）を取得可能な形で公開またはアップロードできる
2. The Packaging Distribution System shall 公開された成果物に、対象 OS／アーキテクチャおよびバリアント（CPU 既定／任意 CUDA）を識別できるメタデータまたは命名を付与する
3. If 必須成果物のいずれかの組み立てまたは検証が失敗する, the Packaging Distribution System shall 当該リリースを成功扱いにしない
4. The Packaging Distribution System shall 埋め込み HTTP バイナリを上流 artifact `release-embedded-http-api`（または合意した同一ジョブ）から消費し、model-embedding の `release-embedded-cli` を再定義しない

### Requirement 9: 導入・起動ドキュメント

**Objective:** As an 運用オペレータ, I want 導入と起動の手順が文書化されている, so that 環境差分（Windows ローカル／Linux クラウド／パッケージ）ごとに迷わず設定できる

#### Acceptance Criteria

1. The Packaging Distribution System shall コンテナイメージ・Windows インストーラ・Linux パッケージそれぞれについて、導入手順と起動手順を文書化する
2. The Packaging Distribution System shall インストール後にサンプル ini でライセンス URL 等を設定する手順を文書化する
3. The Packaging Distribution System shall 対象を Windows x86_64 および Linux x86_64 に限定していることを文書上で明示する
4. Where 上流 `api-console-ui` によりコンソール UI が同一バイナリに含まれる, the Packaging Distribution System shall 導入またはスモーク文書にコンソール正本 URL（`/ui/`）と追加フロント成果物が不要である旨を記載する

### Requirement 10: 責務境界の明確化

**Objective:** As a ステークホルダー, I want 本仕様が担わない責務が明確である, so that 上流仕様や別プロジェクトと重複・衝突しない

#### Acceptance Criteria

1. The Packaging Distribution System shall 本仕様の範囲でライセンスサーバー本体を配布・実装しない
2. The Packaging Distribution System shall 本仕様の範囲で HTTP API のリソース設計・エラー契約・ini キー意味を再設計しない
3. The Packaging Distribution System shall 本仕様の範囲でモデル埋め込みロジックを実装せず、上流の埋め込み成果を消費する
4. The Packaging Distribution System shall クラウド IaC の本格整備および高度な自動更新チャネルを必須成果としない
