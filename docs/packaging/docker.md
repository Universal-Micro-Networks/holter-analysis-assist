# コンテナイメージの導入・起動

Holter HTTP API を Linux クラウド向けコンテナイメージとして導入する手順です。

## 対象環境

- **OS / アーキテクチャ**: **Linux x86_64（amd64）のみ**
- **配布バリアント**: **CPU 既定**が必須成果物です。CUDA 付きは任意の別イメージ／別タグとし、本手順の既定ではありません
- Windows 向けは [windows.md](./windows.md)、非コンテナの Linux パッケージは [linux-packages.md](./linux-packages.md) を参照してください

本仕様の配布対象は **Windows x86_64** と **Linux x86_64** に限定されています。

## 前提

- 上流 CI artifact `release-embedded-http-api` 由来の**埋め込みモデル付き** `holter-http-api` をステージング済みであること
- 生モデル（`.onnx` 等）はイメージに同梱しません
- ライセンスサーバー本体の構築・運用、HTTP API のリソース／エラー契約、ini キー意味の再定義は**本仕様の範囲外**です（上流 `license-client` / `http-api` または別プロジェクトへ委ねます）

## イメージの取得・ビルド

リリース成果物として公開されたイメージを pull するか、検証済みステージングからビルドします。

```bash
# 例: 公開タグ（CPU 既定）
docker pull <registry>/holter-http-api:<version>-cpu

# またはローカルビルド（staging 検証後）
./packaging/scripts/build-docker.sh \
  --staging packaging/out/staging/linux \
  --tag holter-http-api:<version>-cpu
```

タグに `-cpu` が付くものが CPU 既定です。CUDA 用を提供する場合は別タグ（例: `-cuda`）とし、本既定イメージと混在させません。

## サンプル ini の編集

イメージ内にはサンプル `http.ini.example` が同梱されます（既定パス例: `/app/http.ini.example`）。

1. サンプルをランタイム設定へコピーする（コンテナ起動時にボリュームマウント、または起動前にファイルを用意）
2. 少なくとも次を環境に合わせて編集する
   - `[license]` の **`server_url`** … ライセンスサーバー到達先 URL（キー意味の正本は上流 `config/license.ini.example`）
   - `[http]` の **`bind`** … リッスンアドレス（例: `0.0.0.0:8080`。正本は上流 `config/http.ini.example`）
3. 既定の設定パスは `HOLTER_HTTP_INI=/app/config/http.ini`（または `--config`）です

キー意味・権限注記は上流 example のコメントに従い、本ドキュメントでは再定義しません。

## NOTICE の所在

第三者告知（ORT 等）の `NOTICE` はイメージ内の **`/app/NOTICE`** に配置されます。再配布・監査時はこちらを参照してください。正本はリポジトリの `packaging/NOTICE` です。

## 起動例

サンプル ini を編集した `http.ini` をマウントし、ホストのポートをコンテナの **8080** に転送します。

```bash
docker run --rm \
  -p 8080:8080 \
  -v /path/to/http.ini:/app/config/http.ini:ro \
  -e HOLTER_HTTP_INI=/app/config/http.ini \
  holter-http-api:<version>-cpu
```

起動後、上流 `http-api` のヘルス／解析エンドポイント契約に従って疎通確認してください（契約の詳細は上流ドキュメント）。

## 関連

- 手動スモーク観点（CI 必須外）: [manual-smoke.md](./manual-smoke.md)
- ステージング仕様: `packaging/staging/README.md`
