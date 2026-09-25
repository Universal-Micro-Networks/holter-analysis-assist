# Implementation Plan

- [x] 1. Foundation: 配布共通資産と検証ゲートを用意する
- [x] 1.1 NOTICE 正本と配布用サンプル ini の取り込みを追加する
  - ORT 等の第三者告知を含む NOTICE 正本をリポジトリに置く
  - `[license]` は `config/license.ini.example`（license-client）をコピー／参照する。キー正本を本仕様で所有しない
  - `[http]` は http-api の example をコピー／参照する。必要ならパッケージ時に両ソースから単一ランタイム用サンプルへマージする
  - キー意味は再定義せず、権限注記とプレースホルダは上流またはドキュメント側で示す
  - ライセンスサーバー本体・API 契約変更・埋め込みロジックは追加しない
  - 完了条件: NOTICE が存在し、配布ステージングに上流由来の `[http]` / `[license]` 必須例が含まれる
  - _Boundary: NoticeBundle, PackagingIniSample_
  - _Requirements: 6.1, 6.2, 6.3, 6.4, 6.5, 7.1, 7.2, 10.1, 10.2, 10.3, 10.4_

- [x] 1.2 共通ステージング組み立てと成果物検証を用意する
  - 上流 `release-embedded-http-api` の埋め込み済み HTTP バイナリ・上流サンプル ini（コピー／マージ）・NOTICE を共通レイアウトへ集約する手順を用意する
  - 必須ファイル欠落、および生モデル拡張子（少なくとも `.onnx` 等）の混入を検出して失敗させる検証を用意する
  - 配布成果物が埋め込みバイナリ前提であることをレイアウト仕様上で明示する
  - 完了条件: 正常な入力でステージングが組み立てられ、NOTICE／ini 欠落または禁止ファイル混入で非 0 終了する
  - _Boundary: StagingLayout, ArtifactVerify_
  - _Depends: 1.1_
  - _Requirements: 4.1, 4.2, 4.3, 6.1, 7.1, 7.3, 8.3_

- [ ] 2. Core: 各配布形式のビルダー
- [x] 2.1 (P) Linux クラウド向けコンテナイメージ定義を追加する
  - 埋め込み済みバイナリを COPY するイメージ定義を追加し、サンプル ini と NOTICE を同梱する
  - 生モデルディレクトリや `.onnx` がイメージ文脈に入らないよう除外ルールを設ける
  - CPU 既定タグ方針を文書コメントまたはタグ例で示し、イメージ内フル再ビルド＋モデル注入を既定にしない
  - 完了条件: ステージング入力から Linux x86_64 向けイメージを構築でき、起動手順の前提（設定パス／ポート）が定義されている
  - _Boundary: DockerBuilder_
  - _Depends: 1.2_
  - _Requirements: 1.1, 1.2, 1.4, 4.1, 5.1, 6.1, 7.1_

- [ ] 2.2 (P) Windows 向けインストーラ定義を追加する
  - Inno Setup により HTTP バイナリ・サンプル ini・NOTICE・短縮案内を導入先へ配置するインストーラ定義を追加する
  - Windows Service 登録や MSI／WiX は行わない
  - コンパイル失敗時は非 0 とし、サイレント成功にしない
  - 完了条件: Windows x86_64 向け setup 実行ファイルを生成でき、導入先に必須同梱物が含まれる
  - _Boundary: InnoBuilder_
  - _Depends: 1.2_
  - _Requirements: 2.1, 2.2, 2.4, 4.1, 6.1, 7.1_

- [ ] 2.3 (P) Linux 向け deb と rpm の生成を追加する
  - 共通ステージングから deb と rpm の両方を生成する
  - パッケージ導入後にバイナリ・サンプル ini・NOTICE が所定位置へ配置される
  - アーキテクチャは x86_64／amd64 方針に従う
  - 完了条件: 同一入力から `.deb` と `.rpm` の両方が生成され、必須同梱物を含む
  - _Boundary: FpmBuilder_
  - _Depends: 1.2_
  - _Requirements: 3.1, 3.2, 3.3, 4.1, 6.1, 7.1_

- [ ] 3. Integration: CI 公開配線
- [ ] 3.1 CPU 既定の配布ジョブを CI に配線し成果物を公開する
  - Windows ジョブで Inno Setup、Linux ジョブで fpm（および rpm 生成に必要なツール）を明示インストールする前提をジョブに含める
  - 入力は上流 artifact **`release-embedded-http-api`**（埋め込み `holter-http-api`）。欠落時は packaging 失敗
  - model-embedding の **`release-embedded-cli` を再定義・拡張しない**（消費対象外）
  - 検証成功後のみ Docker／Windows インストーラ／deb／rpm を組み立てる
  - 公開成果物名またはメタデータで OS・arch・cpu バリアントを識別できる
  - CUDA 付きは別ジョブ／別成果物とし必須ゲートに含めない
  - 必須成果物または検証の失敗でリリースを成功扱いにしない
  - 完了条件: CI 成功時に CPU 既定の必須成果物一式が取得可能で、失敗時はジョブが落ちる。ジョブ名（入力 `release-embedded-http-api`／本仕様 packaging）が文書化されている
  - _Boundary: ReleasePublish, ArtifactVerify, DockerBuilder, InnoBuilder, FpmBuilder_
  - _Depends: 2.1, 2.2, 2.3_
  - _Requirements: 5.1, 5.2, 5.3, 5.4, 8.1, 8.2, 8.3, 8.4, 4.3, 7.3_

- [ ] 4. Validation: ドキュメントと検証強化
- [ ] 4.1 (P) 導入・起動ドキュメントを 3 系統で追加する
  - コンテナ／Windows インストーラ／Linux パッケージそれぞれについて導入と起動手順を書く
  - インストール後のサンプル ini 編集（ライセンス URL・リッスン）と NOTICE 所在を記載する
  - 対象が Windows x86_64 と Linux x86_64 に限定であること、CPU 既定／CUDA 任意を明示する
  - API 契約やライセンスサーバー本体手順は再設計・複製せず上流／別プロジェクトへ委ねる
  - 完了条件: 3 系統の文書がリポジトリに存在し、設定・NOTICE・対象 OS が追える
  - _Boundary: InstallDocs_
  - _Depends: 1.1_
  - _Requirements: 1.3, 2.3, 3.4, 5.4, 6.4, 7.2, 9.1, 9.2, 9.3, 10.1, 10.2, 10.3, 10.4_

- [ ] 4.2 成果物検証の自動テストを追加する
  - 正常ステージング成功、NOTICE 欠落失敗、サンプル ini 欠落失敗、禁止拡張子混入失敗を自動で確認する
  - 完了条件: 該当テストまたはスクリプト検証が CI もしくはローカル一発コマンドでパス／意図どおり失敗する
  - _Boundary: ArtifactVerify_
  - _Depends: 1.2_
  - _Requirements: 4.3, 7.3, 8.3_

- [ ]* 4.3 実バイナリでの配布スモーク観点を残す
  - 埋め込み HTTP バイナリが利用可能になったときの確認観点（イメージ起動、インストーラ導入、パッケージ導入、ini 設定後の起動）を文書またはテストコメントに残す
  - 初期 CI 必須にはしない（上流・実ライセンス環境依存）
  - 完了条件: 運用者が追えるスモーク観点がリポジトリ内に残っている
  - _Depends: 3.1, 4.1_
  - _Requirements: 1.2, 2.2, 3.3, 9.1_
