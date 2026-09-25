# Linux パッケージ（deb / rpm）の導入・起動

Holter HTTP API を deb または rpm で Linux ホストへ導入する手順です。

## 対象環境

- **OS / アーキテクチャ**: **Linux x86_64**（deb は **amd64**、rpm は **x86_64**）
- **配布バリアント**: **CPU 既定**が必須成果物です。CUDA 付きは任意の別パッケージとし、本手順の既定ではありません
- コンテナ導入は [docker.md](./docker.md)、Windows は [windows.md](./windows.md) を参照してください

本仕様の配布対象は **Windows x86_64** と **Linux x86_64** に限定されています。

## 前提

- パッケージは埋め込みモデル付き `holter-http-api` を配置します（生モデルは同梱しません）
- 同一リリース入力から **`.deb` と `.rpm` の両方**が提供されます
- ライセンスサーバー本体、HTTP API 契約・ini キー意味の再設計は**範囲外**です（上流 `license-client` / `http-api` または別プロジェクトへ）

## 導入

### deb（Debian / Ubuntu 等）

```bash
sudo dpkg -i holter-http-api_<version>_amd64.deb
# 依存解決が必要な場合の例:
# sudo apt-get install -f
```

### rpm（RHEL / Alma / Fedora 等）

```bash
sudo rpm -Uvh holter-http-api-<version>-1.x86_64.rpm
# または
# sudo dnf install ./holter-http-api-<version>-1.x86_64.rpm
```

パッケージ導入後の配置:

| 内容 | パス |
|------|------|
| バイナリ | `/usr/bin/holter-http-api` |
| NOTICE | `/usr/share/holter-http-api/NOTICE` |
| サンプル ini | `/usr/share/holter-http-api/http.ini.example` |

## サンプル ini の編集

1. サンプルをコピーしてランタイム設定を用意する（例）

   ```bash
   sudo mkdir -p /etc/holter-http-api
   sudo cp /usr/share/holter-http-api/http.ini.example /etc/holter-http-api/http.ini
   sudoedit /etc/holter-http-api/http.ini
   ```

2. 少なくとも次を編集する
   - `[license]` の **`server_url`** … ライセンスサーバー URL（正本: 上流 `config/license.ini.example`）
   - `[http]` の **`bind`** … リッスン指定（例: `0.0.0.0:8080`。正本: 上流 `config/http.ini.example`）

3. キー意味は上流 example のコメントに従い、本ドキュメントでは再定義しません

## NOTICE の所在

第三者告知は **`/usr/share/holter-http-api/NOTICE`** にあります。正本はリポジトリの `packaging/NOTICE` です。

## 起動

```bash
holter-http-api --config /etc/holter-http-api/http.ini
# または
# HOLTER_HTTP_INI=/etc/holter-http-api/http.ini holter-http-api
```

systemd ユニット同梱は本仕様の必須ではありません。常駐化は運用側の方針に従ってください。API 契約の詳細は上流 `http-api` を参照してください。

## コンソール UI（同一バイナリ）

上流 `api-console-ui` により、簡易コンソールは **同一の `holter-http-api` バイナリに埋め込み配信**されます。追加のフロントエンド成果物や別 UI サーバーは不要です。

- **正本 URL**: `http://<host>:<port>/ui/`（例: `http://127.0.0.1:8080/ui/`。Windows と同一経路）
- ブラウザ手動スモーク: [../console-ui-smoke.md](../console-ui-smoke.md)

## 関連

- 手動スモーク観点（CI 必須外）: [manual-smoke.md](./manual-smoke.md)
- fpm スクリプト: `packaging/linux/fpm.sh`
