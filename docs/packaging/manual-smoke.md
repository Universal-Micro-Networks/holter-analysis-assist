# 実バイナリ配布の手動スモーク観点

埋め込み HTTP バイナリ（上流 artifact `release-embedded-http-api`）が利用可能になったときに、運用者／リリース担当が追う**手動確認観点**です。

> **CI 必須ではありません。** 実ライセンス環境・実埋め込みバイナリ・対象 OS ランナーに依存するため、初期 CI ゲートには含めません。任意のリリース前確認として使ってください。

関連の導入手順: [docker.md](./docker.md) / [windows.md](./windows.md) / [linux-packages.md](./linux-packages.md)

## 共通前提

- [ ] 入力が埋め込み `holter-http-api`（生 `.onnx` 等をステージングに置いていない）
- [ ] `packaging/scripts/verify-artifact.sh` が対象ステージングで成功している
- [ ] 成果物が **CPU 既定**であること（CUDA は別成果物なら別チェックリスト）

## 1. コンテナイメージ起動（Linux x86_64）

- [ ] CPU タグのイメージを pull または `build-docker.sh` で構築できる
- [ ] サンプル ini をコピーし `server_url` / `bind` を実環境向けに編集した `http.ini` をマウントできる
- [ ] `docker run` でプロセスが起動し、ホストからポート（例: 8080）へ到達できる
- [ ] コンテナ内 `/app/NOTICE` が参照できる

## 2. Windows インストーラ導入（Windows x86_64）

- [ ] `holter-http-api-setup-<version>-cpu.exe` を実行し、導入先に `holter-http-api.exe`・`http.ini.example`・`NOTICE` が配置される
- [ ] サンプル ini を編集（少なくとも `server_url` / `bind`）したうえで `holter-http-api.exe --config ...` が起動する
- [ ] インストーラ失敗時に失敗と分かる（サイレント成功にならない）

## 3. Linux パッケージ導入（deb / rpm、x86_64／amd64）

- [ ] `.deb` および／または `.rpm` をインストールできる
- [ ] `/usr/bin/holter-http-api`・`/usr/share/holter-http-api/NOTICE`・`http.ini.example` が所定位置にある
- [ ] サンプル ini をコピー編集後、`holter-http-api --config ...` で起動できる

## 4. ini 設定後の起動（全系統共通）

- [ ] `[license] server_url` が到達可能なライセンス環境（または運用で許可されたモック／スキップ可能な検証環境）を指している
- [ ] `[http] bind` が意図したリッスンになっている
- [ ] プロセス起動後、上流 `http-api` の手順に沿ったヘルスまたは最小リクエストで受付可能な状態を確認できる

## 範囲外（本チェックで再設計しない）

- ライセンスサーバー本体のデプロイ手順
- HTTP API のリソース設計・エラー契約・ini キー意味の変更

これらは上流／別プロジェクトの文書に従います。
