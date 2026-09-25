# Requirements Document

## Project Description (Input)
検査会社へ配布する成果物に推論モデルの生ファイルが並ぶと、容易にコピー・流用される。現状はファイルパスからモデルをロードしており、大きな重みは Git 管理外だが配布形態は未整備である。リリースバイナリにモデルを埋め込み、運用者が生モデルファイルを扱わなくても Windows / Linux の両方で推論できるようにする。目標は「人間が読めない／簡単に持ち出せない」ことであり、熟練者によるリバース耐性の完全保証は範囲外とする。ライセンス確認・HTTP API・インストーラ本体は本仕様の対象外である。

## Introduction
本仕様は、ホルター解析支援アプリケーションの推論モデル資産を、配布成果物から生ファイルとして分離し、リリース用バイナリへ埋め込むための振る舞いを定義する。検査会社の運用者はモデルファイルを別途配置せずに解析 CLI を実行でき、開発者は従来どおりファイルパスからのロードを開発用途で利用できる。CPU/CUDA 等の実行プロバイダ選択は既存方針を維持し、本仕様では変更しない。

## Boundary Context
- **In scope**: 埋め込みモデルによる推論ロード経路、リリース／配布向けビルドでのモデル注入、既存解析 CLI（`analyze-ecl` / `infer-window`）が埋め込みモデルで動作すること、開発用ファイルパスロードの維持、配布物に生モデルファイルを含めないこと、バイナリサイズの計測、対象 OS（Windows / Linux x86_64）での同一方針
- **Out of scope**: ライセンスサーバー連携、HTTP API・エンドポイント設計、Docker／インストーラ本体、暗号化鍵ローテーションや高度 DRM、熟練者向け完全な耐リバース保証
- **Adjacent expectations**: 上流の既存 Phase-2 解析・ONNX エクスポート成果を入力とする。実行プロバイダ選択（`runtime-ep-selection`）は独立して維持する。下流の `http-api` / `packaging-distribution` は本仕様の埋め込みロード結果（`ModelSource` / `analyze_ecl_with_source`）を前提にできるが、本仕様はそれらを実装しない。`license-client` は正規入口へのゲート挿入を OWN し、本仕様は利用計上を行わない（調整順: ModelSource 配線 → ゲート挿入）

## Requirements

### Requirement 1: 埋め込みモデルによる推論
**Objective:** As a 検査会社の運用者, I want 配布バイナリ単体で推論モデルが利用できる, so that 生モデルファイルを別途管理・配置しなくても解析できる

#### Acceptance Criteria
1. When 埋め込みモデルを含むリリース用バイナリで解析を実行する, the Holter Analysis Assist shall 外部の生モデルファイルを参照せずに推論を完了する
2. When 埋め込みモデルから推論セッションを構築する, the Holter Analysis Assist shall メモリ上のモデルデータからセッションを構築する
3. The Holter Analysis Assist shall 埋め込みモデル経路での推論結果契約（ラベル・出力形式）を、既存のファイルパスロード経路と同等に保つ

### Requirement 2: 配布物からの生モデル分離
**Objective:** As a 製品配布担当者, I want 配布成果物に生モデルファイルが含まれない, so that モデル資産のカジュアルなコピー・流用を抑えられる

#### Acceptance Criteria
1. When リリース／配布プロファイルの成果物を組み立てる, the Holter Analysis Assist shall 生モデルファイルを配布成果物に同梱しない
2. The Holter Analysis Assist shall モデル保護の目標を「運用者や一般利用者が容易にモデルを持ち出せない」水準とし、熟練者による完全な耐リバースを保証対象に含めない
3. The Holter Analysis Assist shall 本番モデルのバイト列をソース管理リポジトリにコミットしない

### Requirement 3: 開発用ファイルパスロードの維持
**Objective:** As a 開発者, I want 従来どおりパス指定でモデルをロードできる, so that 埋め込みビルドなしでもローカル検証と開発反復ができる

#### Acceptance Criteria
1. Where 開発用のファイルパスロードが有効である, when 利用者がモデルパスを指定して解析 CLI を実行する, the Holter Analysis Assist shall 指定パスのモデルファイルから推論を実行する
2. When 埋め込みビルドと開発用パスロードの両方の経路が利用可能である, the Holter Analysis Assist shall どちらを使うかをビルド設定または実行時の明示的な選択で切り替え可能にする
3. If 開発用パスロードで指定されたモデルファイルが存在しない, the Holter Analysis Assist shall 推論を開始せずに明確なエラーを返す

### Requirement 4: 既存 CLI との互換
**Objective:** As a 解析オペレータ, I want 既存の解析 CLI が埋め込みモデルでも動く, so that 運用手順を大きく変えずに移行できる

#### Acceptance Criteria
1. When 埋め込みモデルを含むバイナリで `analyze-ecl` 相当の解析を実行する, the Holter Analysis Assist shall 既存と同様に解析結果を出力する
2. When 埋め込みモデルを含むバイナリで `infer-window` 相当の窓推論を実行する, the Holter Analysis Assist shall 既存と同様に推論結果を出力する
3. While 埋め込みモデル経路を利用している, the Holter Analysis Assist shall 実行プロバイダ選択（自動／CPU／CUDA 等）の既存振る舞いを維持する

### Requirement 5: ビルド時モデル注入と対象プラットフォーム
**Objective:** As a CI／リリース担当者, I want ビルド時に秘密ストア等からモデルを注入できる, so that Windows と Linux のリリース成果物を同一方針で作成できる

#### Acceptance Criteria
1. When リリース用バイナリをビルドする, the Holter Analysis Assist shall CI または秘密ストアから供給されたモデルバイト列をバイナリへ埋め込む手段を提供する
2. If 埋め込み必須のリリースビルドでモデルバイト列が供給されていない, the Holter Analysis Assist shall 埋め込み欠落を検出してビルドを失敗させる
3. The Holter Analysis Assist shall Windows（x86_64）および Linux（x86_64）のリリースビルドで同一の埋め込み方針を適用可能にする

### Requirement 6: バイナリサイズの可視性
**Objective:** As a リリース担当者, I want 埋め込み後のバイナリ肥大を把握できる, so that 配布サイズの許容判断と記録ができる

#### Acceptance Criteria
1. When 埋め込みモデルを含むリリースビルドが完了する, the Holter Analysis Assist shall 成果物バイナリのサイズを計測・記録できる手段を提供する
2. The Holter Analysis Assist shall モデル埋め込みによりバイナリが 100MB を超える場合があることを許容前提とし、サイズ計測結果を隠さない

### Requirement 7: 範囲外の明確化
**Objective:** As a ステークホルダー, I want 本仕様が担わない責務が明確である, so that ライセンス・API・配布パッケージの仕様と重複・衝突しない

#### Acceptance Criteria
1. The Holter Analysis Assist shall 本仕様の範囲でライセンス有効性確認や利用計上を行わない
2. The Holter Analysis Assist shall 本仕様の範囲で HTTP API エンドポイントを定義・実装しない
3. The Holter Analysis Assist shall 本仕様の範囲で Docker イメージ、Windows インストーラ、Linux パッケージの作成を行わない
