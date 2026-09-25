# Brief: packaging-distribution

## Problem

導入が手作業だと、検査会社・クラウドへの展開コストが高い。  
Windows はインストーラ、Linux クラウドはコンテナ／パッケージで簡単に配りたい。

## Current State

- CI で Linux / Windows の release ビルドはある
- Docker Image、Windows Installer、rpm/deb 等の配布成果物は未整備
- モデルはファイル配布前提のまま（埋め込み後はバイナリ中心に変わる）

## Desired Outcome

- Docker Image で Linux クラウドへ容易にデプロイできる
- Windows Installer でローカル導入できる
- Linux 向け配布ファイル（deb および/または rpm 等）を提供できる
- いずれの成果物にも生モデルファイルを含めない（埋め込みバイナリ前提）
- ライセンス用 ini の配置・サンプルを同梱する

## Approach

`http-api`（および必要なら CLI）の release 成果物を入力に、配布パイプラインを追加する。  
- コンテナ: 公式なマルチステージ Dockerfile  
- Windows: WiX または Inno Setup（要件で選定）  
- Linux: deb/rpm（fpm や native ツール等、要件で選定）  
CI から成果物を公開できる状態にする。

## Scope

- **In**:
  - Dockerfile と実行ドキュメント
  - Windows Installer 成果物
  - Linux パッケージ（少なくとも 1 形式、可能なら deb+rpm）
  - サンプル ini・サービス起動方法の文書
- **Out**:
  - ライセンスサーバーの配布
  - クラウド IaC（Terraform 等）の本格整備（後続可）
  - 自動更新チャネルの高度化（後続可）

## Boundary Candidates

- コンテナイメージ
- Windows インストーラ
- Linux パッケージ
- CI release ジョブ

## Out of Boundary

- API 仕様そのもの
- モデル埋め込みロジック
- ライセンスサーバー

## Upstream / Downstream

- **Upstream**: `http-api`（稼働バイナリ）, `model-embedding`, `license-client`
- **Downstream**: 運用ドキュメント、顧客導入手順

## Existing Spec Touchpoints

- **Extends**: CI workflow（成果物追加）
- **Adjacent**: なし

## Constraints

- 対象: Windows x86_64 / Linux x86_64
- 配布バリアントは CPU 既定（CUDA 付きは任意・別成果物）
- インストール後に ini でライセンス URL を設定できること
- ORT / 第三者ライセンスの NOTICE を成果物に含める
