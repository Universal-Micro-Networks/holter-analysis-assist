# Implementation Plan

- [x] 1. Foundation: 埋め込みビルドゲートを用意する
- [x] 1.1 埋め込み用 Cargo feature とビルド時モデル注入ゲートを追加する
  - 配布／CI 向けに、開発 default とは独立した埋め込み feature を定義する
  - feature 有効時はビルド入力としてモデルファイルパス環境変数を必須にし、未設定・空・非ファイルならビルドを失敗させる
  - feature 無効時は従来どおりパスロードのみでビルドできることを確認する
  - 完了条件: `embedded-model` feature 有効かつモデル未供給でビルドが失敗し、供給時は成功する
  - _Boundary: EmbedBuildGate_
  - _Requirements: 5.1, 5.2_

- [x] 1.2 埋め込みモデルバイトへの実行時アクセスを提供する
  - feature 有効時のみ静的バイト列としてモデルデータへアクセスできるモジュールを追加する
  - feature 無効時は埋め込みソースを選択できない（コンパイルまたは明確な実行時エラー）
  - 本番モデルファイル自体はリポジトリに追加しない
  - 完了条件: feature 有効ビルドで埋め込みバイトが非空で参照でき、feature 無効ビルドでは埋め込み経路が使えない
  - _Boundary: EmbeddedModelBytes_
  - _Depends: 1.1_
  - _Requirements: 1.1, 2.3, 5.1_

- [ ] 2. Core: モデルソース契約とメモリロードを実装する
- [x] 2.1 パス／埋め込みを区別するモデルソース契約を追加する
  - 解析・CLI が共通して参照できるロード元の値オブジェクトを導入する
  - 開発用 Path と配布用 Embedded の切替意図が型上で明示される
  - 完了条件: Path と Embedded を選択できる公開契約がライブラリから利用できる
  - _Boundary: ModelSource_
  - _Requirements: 3.1, 3.2_

- [ ] 2.2 メモリおよびソース指定から推論セッションを構築できるようにする
  - 既存の `load_with_provider`（Path）と EP 選択方針を維持したまま、メモリバイトからのセッション構築を追加する
  - ソース指定 API（`load_from_source`）で Path はファイル、Embedded は埋め込みバイトへ解決し、EP 解決は Path 経路と共有する
  - 欠落パス・空バイトは推論開始前に明確なエラーとなる
  - Path 経路と Embedded 経路で推論結果契約（ラベル・出力形式）が同等である
  - 完了条件: 埋め込みバイトのみで Phase-2 セッションが構築でき、既存 EP 指定が両経路で動作する
  - _Boundary: Phase2ModelLoader_
  - _Depends: 1.2, 2.1_
  - _Requirements: 1.1, 1.2, 1.3, 3.3, 4.3_

- [ ] 3. Integration: 解析パイプラインと CLI をソース切替対応にする
- [ ] 3.1 ECL 解析エントリをモデルソース対応にする
  - 正規公開入口 `analyze_ecl_with_source` を追加し、`ModelSource` 経由でロードする
  - 既存の `analyze_ecl` / `analyze_ecl_with_limit` は Path → `ModelSource::Path` の薄い互換ラッパとする
  - ライセンス meter の挿入は本タスクに含めない（`license-client` が正規入口へ後から追加）
  - 完了条件: 埋め込みソース指定で `analyze_ecl_with_source` 相当の解析が外部生モデルなしに完了する
  - _Boundary: AnalyzeEntry_
  - _Depends: 2.2_
  - _Requirements: 1.3, 4.1_

- [ ] 3.2 CLI の既定モデルソース解決を実装する
  - `--model` 指定時は常に Path を使う
  - 未指定かつ埋め込みビルドでは Embedded を使う
  - 未指定かつ非埋め込みビルドでは現行の開発用デフォルトパスを使う
  - `analyze-ecl` と `infer-window` の両方に同一解決を適用する
  - 完了条件: 埋め込みバイナリで引数未指定の両コマンドが動作し、パス指定時はファイル優先となる
  - _Boundary: CliModelSelect_
  - _Depends: 3.1_
  - _Requirements: 3.1, 3.2, 4.1, 4.2_

- [ ] 4. CI: 注入付き release とサイズ可視化を追加する
- [ ] 4.1 Windows／Linux でモデル注入 release・サイズ計測・生モデル非同梱を自動化する
  - CI ジョブ／アーティファクト名を `release-embedded-cli` とし、埋め込み **CLI** バイナリのみを対象にする
  - CI が秘密ストア／secret から一時モデルを配置し、埋め込み feature 付き release を両ターゲットで行う
  - release 成果物バイナリのサイズをログ等に記録し、100MB 超でも抑制しない
  - アップロード成果物に生モデルファイルを含めない
  - packaging ジョブ・`holter-http-api` 埋め込み成果物・ライセンス／インストーラ作成ステップを本変更に追加しない（下流 OWN）
  - 完了条件: 両 OS の `release-embedded-cli` アーティファクトとサイズ記録が得られ、artifact に `.onnx` 等が含まれない
  - _Boundary: CiEmbedRelease_
  - _Depends: 1.1, 3.2_
  - _Requirements: 2.1, 2.2, 2.3, 5.3, 6.1, 6.2, 7.1, 7.2, 7.3_

- [ ] 5. Validation: 経路切替と欠落時挙動を検証する
- [ ] 5.1 モデルソース解決とロード失敗の自動テストを追加する
  - Path／Embedded の解決、空バイト・欠落パスのエラーを検証する
  - 可能なら最小 fixture バイトによる埋め込みセッション構築を feature 付きで検証する
  - 完了条件: 関連テストが CI またはローカルでパスし、失敗ケースが明示的にカバーされる
  - _Boundary: Phase2ModelLoader, ModelSource_
  - _Depends: 2.2, 3.2_
  - _Requirements: 1.2, 3.2, 3.3_

- [ ] 5.2 CLI の埋め込み／パス優先の結合確認を行う
  - 埋め込みビルドで `--model` 未指定の解析が成功すること
  - `--model` に欠落パスを与えた場合は非ゼロ終了と明確メッセージになること
  - 完了条件: 上記 CLI シナリオが再現手順付きで確認できる（自動テストまたはスクリプト化）
  - _Boundary: CliModelSelect_
  - _Depends: 3.2, 5.1_
  - _Requirements: 3.1, 3.3, 4.1, 4.2_

- [ ]* 5.3 埋め込み有無のバイナリサイズ差分記録を補強する
  - 同一ターゲットで feature 有無のサイズを比較ログできる場合に追加する
  - 要件 6 の受け入れは 4.1 の計測で満たす。本タスクは差分可視化の任意強化
  - 完了条件: 差分ログ手順または CI ステップが追加されている
  - _Boundary: CiEmbedRelease_
  - _Depends: 4.1_
  - _Requirements: 6.1, 6.2_
