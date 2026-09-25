# Windows インストーラの導入・起動

Holter HTTP API を Windows 向けインストーラで導入する手順です。

## 対象環境

- **OS / アーキテクチャ**: **Windows x86_64 のみ**
- **配布バリアント**: **CPU 既定**が必須成果物です（インストーラ名例: `holter-http-api-setup-<version>-cpu.exe`）。CUDA 付きは任意の別成果物とし、本手順の既定ではありません
- Linux コンテナは [docker.md](./docker.md)、Linux パッケージは [linux-packages.md](./linux-packages.md) を参照してください

本仕様の配布対象は **Windows x86_64** と **Linux x86_64** に限定されています。

## 前提

- インストーラは埋め込みモデル付き `holter-http-api.exe` を配置します（生 `.onnx` は同梱しません）
- Windows Service 登録や MSI／WiX は行いません。プロセスは運用者が手動起動します
- ライセンスサーバー本体、HTTP API 契約・ini キー意味の再設計は**範囲外**です（上流 `license-client` / `http-api` または別プロジェクトへ）

## 導入

1. リリース成果物の `holter-http-api-setup-<version>-cpu.exe` を入手する
2. 管理者権限でインストーラを実行する（失敗時はサイレント成功になりません）
3. 既定の導入先は Program Files 配下の `Holter HTTP API` です

導入先に少なくとも次が配置されます。

| 内容 | 配置例 |
|------|--------|
| バイナリ | `{app}\bin\holter-http-api.exe` |
| サンプル ini | `{app}\http.ini.example` |
| NOTICE | `{app}\NOTICE` |
| 短縮ドキュメント | `{app}\docs\` |

## サンプル ini の編集

1. `{app}\http.ini.example` をコピーし、実行時設定ファイルを用意する（例: `{app}\config\http.ini`）
2. 少なくとも次を編集する
   - `[license]` の **`server_url`** … ライセンスサーバー URL（正本: 上流 `config/license.ini.example`）
   - `[http]` の **`bind`** … リッスン指定（例: `0.0.0.0:8080`。正本: 上流 `config/http.ini.example`）
3. 必要に応じて上流 example のコメントに従い `api_key` 等を設定する（キー意味は再定義しない）

## NOTICE の所在

第三者告知は導入先の **`{app}\NOTICE`**（インストーラ既定では `Holter HTTP API` フォルダ直下）にあります。正本はリポジトリの `packaging/NOTICE` です。

## 起動

コマンドプロンプトまたは PowerShell で、設定ファイルを指定して起動します。

```bat
cd "C:\Program Files\Holter HTTP API"
bin\holter-http-api.exe --config config\http.ini
```

環境変数 `HOLTER_HTTP_INI` で設定パスを指定することもできます（上流 http-api の解決規則に従います）。

起動後の API 利用方法は上流 `http-api` ドキュメントを参照してください。

## 関連

- 手動スモーク観点（CI 必須外）: [manual-smoke.md](./manual-smoke.md)
- Inno 定義: `packaging/windows/holter-http-api.iss`
