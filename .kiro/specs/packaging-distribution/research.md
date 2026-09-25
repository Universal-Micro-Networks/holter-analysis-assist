# Research & Design Decisions: packaging-distribution

## Summary
- **Feature**: `packaging-distribution`
- **Discovery Scope**: New Feature（既存 CI release バイナリを入力とする配布パイプライン追加）
- **Key Findings**:
  - 既存 CI は Linux / Windows の release バイナリを artifact 化するのみで、Docker / Installer / deb・rpm は未整備
  - 埋め込みモデルはビルド時注入のため、コンテナは「ソースから再ビルド」より「埋め込み済みバイナリを COPY」が安全（秘密モデルを Docker ビルド層へ漏らさない）
  - Windows インストーラは Inno Setup を採用（単純なファイル配置＋サンプル ini／NOTICE に十分。WiX/MSI は後続可）
  - Linux は fpm で deb＋rpm の両方を同一ステージングから生成可能
  - ORT 再配布には NOTICE / ThirdPartyNotices 相当の同梱が必要

## Research Log

### 既存コードベースと上流契約
- **Context**: 配布入力と境界の確認
- **Sources Consulted**: `.github/workflows/ci.yml`、`.kiro/specs/http-api/`、`.kiro/specs/model-embedding/`、`.kiro/specs/license-client/`、roadmap / brief
- **Findings**:
  - CI は `holter-analysis-assist` CLI バイナリのみアップロード。HTTP バイナリ（`holter-http-api`）は上流 `http-api` 実装後に追加される前提
  - `[license]` 必須 `server_url`、`[http]` 必須 `bind`。サンプル ini はキー意味を再定義しない
  - 生 `.onnx` を成果物に含めない契約は `model-embedding` が定義。本仕様は組み立て検証で再確認する
- **Implications**: 配布の正本入力は埋め込み済み HTTP バイナリ＋サンプル ini＋NOTICE。CLI 同梱は任意だが必須ではない（要件は HTTP サービス導入）

### Windows インストーラ: WiX vs Inno Setup
- **Context**: brief が WiX または Inno の選定を要求
- **Sources Consulted**:
  - [WiX vs Inno 比較](https://www.advancedinstaller.com/versus/wix-toolset/wix-toolset-vs-inno-setup-packaging-tool.html)
  - GitHub Actions WiX v4（`dotnet tool install --global wix`）事例
  - runner-images: Inno Setup は windows-2025 で一時削除→明示インストール（choco/winget）が必要
- **Findings**:
  - WiX: MSI・企業展開（GPO/SCCM）に強いが、単純配置には記述量が大きい
  - Inno: EXE インストーラ。スクリプトが短く、バイナリ＋設定＋NOTICE 配置に適合。サービス登録は必須ではない
  - 本製品 MVP は「導入→ini 設定→プロセス起動」であり、Windows Service / MSI 必須ではない
- **Implications**: Inno Setup を選定。企業向け MSI は Non-Goal／後続

### Linux パッケージ: fpm による deb + rpm
- **Context**: 少なくとも 1 形式、可能なら両方
- **Sources Consulted**: [fpm getting started](https://fpm.readthedocs.io/en/stable/getting-started.html)、[action-fpm](https://github.com/marketplace/actions/action-fpm)、deb/rpm CI 事例
- **Findings**:
  - `fpm -s dir -t deb|rpm` で同一ステージングツリーから両形式を生成できる
  - ubuntu-latest 上で `gem install fpm`＋rpm ツールが一般的
- **Implications**: fpm で deb と rpm の両方を必須成果物とする（実行可能と判断）

### Docker 実行イメージ戦略
- **Context**: Linux クラウド向けイメージと埋め込みモデル
- **Sources Consulted**: Rust multi-stage / distroless 実践、cargo-chef 文書
- **Findings**:
  - マルチステージ＋最小ランタイム（debian slim または distroless/cc）が標準
  - 埋め込みモデルは CI 秘密注入。イメージ内フルビルドはモデル供給とキャッシュ汚染のリスクが高い
  - ライセンスクライアントは HTTPS 外向き通信が必要 → CA 証明書付きランタイムが必要
- **Implications**: 既定は「CI が作った埋め込み CPU バイナリを COPY」。ランタイムは `debian:bookworm-slim`（デバッグ容易・CA 付き）。distroless は任意最適化として後続可

### ORT / 第三者 NOTICE
- **Context**: Requirement 7
- **Sources Consulted**: ONNX Runtime `ThirdPartyNotices.txt`、Apache-2.0 NOTICE 再配布条項
- **Findings**:
  - バイナリ再配布時はライセンス文と第三者告知の可读コピーが必要
  - リポジトリに集約 `NOTICE`（自社帰属＋ORT／依存の参照または抜粋）を置き、全成果物へ同梱する
- **Implications**: `packaging/NOTICE`（または `NOTICE`）を正本とし、検証スクリプトで欠落を fail

## Architecture Pattern Evaluation

| Option | Description | Strengths | Risks / Limitations | Notes |
|--------|-------------|-----------|---------------------|-------|
| Artifact-first packaging pipeline | CI release バイナリを入力に Docker/Inno/fpm が組み立て | 埋め込み秘密と整合、再現性 | HTTP バイナリ未実装時は依存待ち | 採用 |
| Source-rebuild in each packager | 各成果物が独自に cargo build | 単体完結 | モデル注入重複、成果の不一致 | 却下 |
| Monolithic release script only | 1 巨大スクリプト | 単純 | 境界不明・並列困難 | 却下 |

## Design Decisions

### Decision: Windows インストーラに Inno Setup を採用
- **Context**: WiX または Inno の選定必須
- **Alternatives Considered**:
  1. WiX Toolset v4 — MSI、企業展開向き
  2. Inno Setup — EXE、単純配置向き
- **Selected Approach**: Inno Setup 6.x。`packaging/windows/holter-http-api.iss` でバイナリ・サンプル ini・NOTICE・起動ドキュメントを Program Files 相当へ配置
- **Rationale**: MVP はサービス登録や MSI 必須ではなく、導入コスト低減が主目的。記述量と CI 明示インストール（choco）で十分
- **Trade-offs**: MSI/GPO 未対応。必要なら後続で WiX 追加可
- **Follow-up**: インストーラ失敗時に非 0 終了／ログが残ることを CI または手動手順で確認

### Decision: Linux は fpm で deb と rpm の両方
- **Context**: Requirement 3.2（両方可能なら両方）
- **Alternatives Considered**:
  1. deb のみ（dpkg-deb）
  2. nfpm / cargo-deb 等
  3. fpm で deb+rpm
- **Selected Approach**: ステージングディレクトリを組み立て、fpm で `-t deb` と `-t rpm` を実行
- **Rationale**: 同一レイアウトから両形式を得られる。CI 実績が多い
- **Trade-offs**: Ruby gem 依存。ネイティブ packaging よりメタデータ制御は粗いが本要件には十分
- **Follow-up**: パッケージ名 `holter-http-api`、アーキテクチャ amd64/x86_64

### Decision: Docker はバイナリ COPY 型マルチステージ
- **Context**: 生モデル非同梱＋埋め込み注入
- **Alternatives Considered**:
  1. イメージ内 cargo build + BuildKit secret
  2. 事前ビルドバイナリを COPY（採用）
- **Selected Approach**: Dockerfile の builder 相当は「外部から渡された release バイナリを受け取るステージ」または CI が `docker build --build-context` / COPY でバイナリを注入。ランタイムにバイナリ・サンプル ini・NOTICE のみ
- **Rationale**: モデル秘密をイメージ履歴へ残しにくく、`http-api` / `model-embedding` の release と 1:1 対応
- **Trade-offs**: ローカル `docker build` 単体ではバイナリ供給が必要（文書化）
- **Follow-up**: `.dockerignore` で `resources/models/**/*.onnx` を明示除外し、検証で確認

### Decision: CPU 既定必須、CUDA は別ジョブ任意
- **Context**: Requirement 5
- **Selected Approach**: 必須 CI 成果は CPU 既定のみ。CUDA 付きは別 workflow/job・別タグ（例: `-cuda`）で任意
- **Rationale**: roadmap / brief の制約に一致。GPU ランナーコストを必須ゲートから外す
- **Trade-offs**: CUDA 利用者は別手順。初期はドキュメントのみでも可（成果物自体は任意）

### Decision: 共通ステージングと検証ゲート
- **Context**: 全成果物で ini / NOTICE / 生モデル非同梱を揃える
- **Selected Approach**: `packaging/staging/` レイアウト仕様＋`packaging/scripts/verify-artifact.sh`（または相当）で (1) NOTICE 存在 (2) サンプル ini 存在 (3) `*.onnx` 等の禁止拡張子なし を検査。失敗時はリリース失敗
- **Rationale**: Requirement 4.3 / 7.3 / 8.3 を機械検証可能にする
- **Trade-offs**: 禁止リストは拡張子ベース（完全保証ではない）。埋め込みバイトは対象外（意図どおり）

## Risks & Mitigations
- HTTP バイナリ未実装 — 上流 `http-api` 完了を前提。本仕様タスクはバイナリ名 `holter-http-api` を固定
- 埋め込みビルド欠落 — 配布ジョブは埋め込み feature 付き artifact のみ受理
- Inno が runner に無い — choco で明示インストール
- 巨大バイナリ（100MB+） — artifact サイズ・Docker 層サイズを文書化し隠さない
- CUDA 任意の未提供が苦情化 — ドキュメントで「CPU 既定／CUDA 任意」を明示

## References
- [fpm documentation](https://fpm.readthedocs.io/en/stable/)
- [ONNX Runtime ThirdPartyNotices](https://github.com/microsoft/onnxruntime/blob/main/ThirdPartyNotices.txt)
- [WiX vs Inno Setup](https://www.advancedinstaller.com/versus/wix-toolset/wix-toolset-vs-inno-setup-packaging-tool.html)
- 上流: `.kiro/specs/http-api/design.md`, `.kiro/specs/model-embedding/design.md`, `.kiro/specs/license-client/design.md`
