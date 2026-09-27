# Windows GPU 端末での速度・精度計測手順（推論高速化）

Windows の NVIDIA GPU（Ampere 世代以降）端末で、推論高速化の設定（まとめ推論・CUDA チューニング）の**速度と精度**を、比較サブコマンド `compare-accel` のレポートで計測する手順です。実 GPU・実 ECL・ライセンス環境に依存するため、**CI 必須にはしません**。

関連: 設定項目の説明は [README の「推論の高速化設定」](../../README.md)、各キーの既定値は `config/http.ini.example`、可変バッチモデルの作り方は [`tools/export/README.md`](../../tools/export/README.md) の「可変バッチでの再エクスポート（まとめ処理用）」。

採否の閾値はこの手順では決めません。ここで記録した計測結果を見て別途決めます。

## 1. 前提

- [ ] NVIDIA GPU（Ampere 世代以降）とドライバが導入済み（`nvidia-smi` で GPU が見える）
- [ ] CUDA Toolkit ≥ 13.2（例: `C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.4`）
- [ ] cuDNN ≥ 9.23（例: `C:\Program Files\NVIDIA\CUDNN\v9.26\bin\13.4\x64`）
- [ ] ライセンス設定 `config/license.ini`（`compare-accel` は **ECL あたり 2 回**計上する。実行回数 × ECL 数 × 2 回分の計上を見込む）
- [ ] 計測に使う実 ECL（丸 1 日分を 1 件以上。可能なら複数件）
- [ ] 本番モデル `resources/models/phase2_rev1.onnx`（固定バッチ 1）と、可変バッチで再エクスポートしたモデル（例: `resources/models/phase2_rev1_dynamic.onnx`）

### 1.1 ビルド

GPU 計測には CUDA 対応の開発ビルド（既定機能 `cuda` を含む）を使います。配布物（インストーラ・Docker・Linux パッケージ）は CPU ビルドのため GPU 計測には使えません。

```powershell
cargo build --release
# HTTP の確認（4.5）で holter-http-api.exe を使う場合
cargo build --release --bin holter-http-api
```

`target\release\holter-analysis-assist.exe` ができていることを確認します。ビルド済みの exe を実行するだけなら Visual Studio の開発者環境（`VsDevCmd.bat`）は要りません。

### 1.2 可変バッチモデル

本番モデル `phase2_rev1.onnx` は固定バッチ 1 のため、`--batch-size` を指定しても実効まとめ件数は 1 のままで、`warning: model has fixed batch 1; batch_size=16 is ignored` が出ます。まとめ推論を計測するには、`tools/export/README.md` の手順で可変バッチモデルを作り、`--model`（ラッパでは `-Model`）で指定します。

```powershell
$env:PYTHONPATH = "tools"
python tools/export/export_onnx.py `
  --weights resources/models/phase2_internal_finetuned_eventstrong_noise_20s10s_rev1.weights.h5 `
  --output resources/models/phase2_rev1_dynamic.onnx `
  --dynamic-batch --verify-batch 4
```

## 2. 実行ラッパ `tools/compare/run_accel_compare.ps1`

CUDA / cuDNN のパスをこのプロセスだけに通して、ビルド済みの `compare-accel` を実行します。終了コードは `compare-accel` のもの（0 = 成功、1 = エラー、2 = 閾値不合格または引数エラー）をそのまま返します。

```powershell
# ヘルプ
Get-Help .\tools\compare\run_accel_compare.ps1 -Detailed

# 例: CUDA・まとめ 16・Conv1D パディング有効
.\tools\compare\run_accel_compare.ps1 -Ecl D:\ecl\a.ecl,D:\ecl\b.ecl `
  -Model resources\models\phase2_rev1_dynamic.onnx `
  -Provider cuda -BatchSize 16 -CudaConv1dPad on

# 実行せずにコマンドと PATH 設定だけ確認する
.\tools\compare\run_accel_compare.ps1 -Ecl D:\ecl\a.ecl -Provider cuda -DryRun
```

| パラメータ | 既定 | 内容 |
|---|---|---|
| `-Ecl` | （必須） | 比較する ECL（カンマ区切りで複数可） |
| `-Provider` | `cuda` | 候補の実行プロバイダ（`auto` / `cpu` / `cuda`） |
| `-BatchSize` | 指定なし（CLI 既定 16） | `--batch-size` |
| `-CudaTf32` / `-CudaConv1dPad` / `-CudaGraph` | 指定なし | `--cuda-tf32` / `--cuda-conv1d-pad-to-nc1d` / `--cuda-graph`（`true` / `false` / `on` / `off` / `1` / `0`） |
| `-Model` | 指定なし（CLI 既定） | `--model` |
| `-ReportDir` | `<リポジトリ>\output\accel_compare\<日時>` | `--report-dir`。省略時は実行ごとに新しいフォルダを作る |
| `-ToleranceSamples` / `-ProbStride` / `-MaxWindows` | 指定なし | `--tolerance-samples` / `--prob-stride` / `--max-windows` |
| `-MinRhythmWindowAgreement` / `-MinBeatMatchRate` / `-MinBeatClassAgreement` / `-MaxProbAbsDiff` / `-MaxOffsetSamples` | 指定なし | 閾値 5 種（`--flag=値` の形で渡す） |
| `-LicenseConfig` | 指定なし | `--license-config` |
| `-Exe` | `<リポジトリ>\target\release\holter-analysis-assist.exe` | 実行する exe |
| `-CudaPath` | `C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.4` | CUDA Toolkit のフォルダ |
| `-CudnnBin` | `C:\Program Files\NVIDIA\CUDNN\v9.26\bin\13.4\x64` | cuDNN の DLL フォルダ |
| `-WithoutCuda` | オフ | CUDA / cuDNN を PATH に足さず、既存の PATH からも取り除く（4.3 の確認用） |
| `-DryRun` | オフ | 実行せずにコマンドを表示する |

ラッパが扱わない引数は `-ExtraArgs @('--flag=値')` で追加できます。

- 相対パス（`-Ecl` / `-Model` / `-ReportDir` / `-LicenseConfig` / `-Exe`）は実行したフォルダを基準に解決します。`compare-accel` 自体はリポジトリのルートで実行するため、`--model` 省略時の既定モデルや `config/license.ini` はリポジトリ内のものが使われます。
- スクリプトの実行ポリシーで止められる場合は `powershell -ExecutionPolicy Bypass -File .\tools\compare\run_accel_compare.ps1 -Ecl a.ecl,b.ecl ...` の形で実行します（`-File` 経由でもカンマ区切りの `-Ecl` は複数として扱います）。

## 3. レポートの見方

`--report-dir` の下に次が出ます。

| パス | 内容 |
|---|---|
| `report.md` | 人向け要約（標準出力にも表示）。設定・判定・集計・ファイル別の精度表と速度表 |
| `report.json` | 機械可読レポート。`aggregate.accuracy`、`aggregate.baseline_perf` / `candidate_perf`、`aggregate.speedup_total` / `speedup_inference`、`verdict` など |
| `<ECL のファイル名（拡張子なし）>\baseline.csv`、`candidate.csv` | ECL ごとの基準・候補の解析結果 |

- 「設定」表の実行プロバイダ・まとめ件数は**解決後の実効値**です（要求値と異なる場合は併記）。GPU を計測したつもりで CPU になっていないか、まとめ件数がモデルの固定バッチに落ちていないかを最初に確認します。
- 速度は `windows_per_s_inference`（推論段階基準）と `windows_per_s_total`（総時間基準）、候補 ÷ 基準の `speedup_inference` / `speedup_total` を見ます。標準エラーの `compare: baseline perf: ...` / `compare: candidate perf: ...` 行にも段階別時間が出ます。
- 分母が 0 の率（例: 対応拍が 0 件のときのラベル一致率、候補が 0 拍のときの候補比対応率）は指標上 1.0 として扱い、`report.md` の精度表では「比較対象なし」と表示します。一方、判定表ではその指標が `1.000000` で「合格」と表示されることがあります。基準に拍があって候補が 0 拍なら基準比対応率が 0 になるので異常は隠れませんが、閾値を使うときは `--min-beat-match-rate` だけにせず、`--min-rhythm-window-agreement` や `--max-prob-abs-diff` など他の閾値と組み合わせてください。
- 閾値を 1 つも指定しなければ判定はしません（終了コード 0）。

## 4. 計測項目

各項目は**実行ごとに新しい `--report-dir`** を使い、結果を 6 章の記録表に残します（ラッパは `-ReportDir` 省略時に自動で新しいフォルダを作ります）。

### 4.1 CPU でまとめ件数 1 と 16 の結果が一致すること

基準（CPU・まとめ 1）と候補（CPU・まとめ 16）で、拍の位置・拍ラベル・リズム区間が一致することを確認します。可変バッチモデルを使います（固定バッチ 1 のモデルでは候補もまとめ 1 になり、確認になりません）。

```powershell
.\tools\compare\run_accel_compare.ps1 -Ecl D:\ecl\a.ecl `
  -Model resources\models\phase2_rev1_dynamic.onnx `
  -Provider cpu -BatchSize 16 `
  -MinBeatMatchRate 1 -MinBeatClassAgreement 1 -MinRhythmWindowAgreement 1
```

- [ ] 同じ内容を CLI で直接実行する場合は `compare-accel --provider cpu --batch-size 16 --model <可変バッチモデル> <ECL>`
- [ ] 「設定」表で候補のまとめ件数が 16、モデルのバッチ形状が `dynamic`
- [ ] 検出拍数の差が 0、対応率（基準比・候補比）100%、拍ラベル一致率 100%、リズム区間の一致率 100%
- [ ] 判定が「合格」、終了コード 0
- [ ] 出力確率の最大絶対差（浮動小数点の丸め程度のはず）を記録する

### 4.2 CUDA の設定組合せごとの速度と精度

候補を CUDA にして、まとめ件数と CUDA チューニングの組合せごとに計測します。`auto` は CUDA が使えないと標準エラーの 1 行だけで CPU に切り替わって比較を続けるため、計測では `-Provider cuda` を明示します。

| # | まとめ件数 | `cuda_tf32` | `cuda_conv1d_pad_to_nc1d` | `cuda_graph` | モデル |
|---|---|---|---|---|---|
| C1 | 1 | 未指定 | 未指定 | 未指定 | 固定バッチ 1（本番モデル）※導入前相当 |
| C2 | 1 | 未指定 | 未指定 | 未指定 | 可変バッチ |
| C3 | 8 | 未指定 | 未指定 | 未指定 | 可変バッチ |
| C4 | 16 | 未指定 | 未指定 | 未指定 | 可変バッチ |
| C5 | 32 | 未指定 | 未指定 | 未指定 | 可変バッチ |
| C6 | 16 | 未指定 | `on` | 未指定 | 可変バッチ |
| C7 | 16 | `false` | 未指定 | 未指定 | 可変バッチ |
| C8 | 16 | 未指定 | 未指定 | `on` | 可変バッチ |
| C9 | 16 | 未指定 | `on` | `on` | 可変バッチ |

```powershell
$ecl = "D:\ecl\a.ecl"
$dyn = "resources\models\phase2_rev1_dynamic.onnx"
$cases = @(
    @{ Name = "C1"; Args = @{ Model = "resources\models\phase2_rev1.onnx"; BatchSize = 1 } },
    @{ Name = "C2"; Args = @{ Model = $dyn; BatchSize = 1 } },
    @{ Name = "C3"; Args = @{ Model = $dyn; BatchSize = 8 } },
    @{ Name = "C4"; Args = @{ Model = $dyn; BatchSize = 16 } },
    @{ Name = "C5"; Args = @{ Model = $dyn; BatchSize = 32 } },
    @{ Name = "C6"; Args = @{ Model = $dyn; BatchSize = 16; CudaConv1dPad = "on" } },
    @{ Name = "C7"; Args = @{ Model = $dyn; BatchSize = 16; CudaTf32 = "false" } },
    @{ Name = "C8"; Args = @{ Model = $dyn; BatchSize = 16; CudaGraph = "on" } },
    @{ Name = "C9"; Args = @{ Model = $dyn; BatchSize = 16; CudaConv1dPad = "on"; CudaGraph = "on" } }
)
$stamp = Get-Date -Format "yyyyMMdd-HHmmss"
foreach ($c in $cases) {
    $a = $c.Args
    .\tools\compare\run_accel_compare.ps1 -Ecl $ecl -Provider cuda @a `
        -ReportDir "output\accel_compare\$stamp\$($c.Name)"
    "{0}: exit {1}" -f $c.Name, $LASTEXITCODE
}
```

- [ ] 各ケースで「設定」表の候補の実行プロバイダが `cuda`、まとめ件数が指定どおり（C1 のみ 1・`fixed:1`）
- [ ] 各ケースの `speedup_inference` / `speedup_total`、候補の `windows_per_s_inference` を記録する
- [ ] 各ケースの精度（対応率・拍ラベル一致率・リズム区間の一致率・出力確率の最大絶対差・位置ずれ最大）を記録する
- [ ] まとめ件数を上げたときの GPU メモリ使用量を別ウィンドウの `nvidia-smi -l 1` で記録する（メモリ不足は `ort error: ...` として解析失敗になる）
- [ ] 丸 1 日 ECL で、C4（可変バッチ 16）と C2（まとめ 1）の推論段階時間を比較して記録する（目標値は設けない）

### 4.3 候補の実行プロバイダが使えないとき比較が中止されること

CUDA が使えない状態で候補に `cuda` を指定し、解析を始める前に中止されることを確認します。ラッパの `-WithoutCuda` は CUDA / cuDNN を PATH に足さず、既存の PATH からも取り除きます（ドライバは残るが CUDA 実行時ライブラリが読めなくなる）。GPU のない端末でも確認できます。

```powershell
$dir = "output\accel_compare\no-cuda-$(Get-Date -Format yyyyMMdd-HHmmss)"
.\tools\compare\run_accel_compare.ps1 -Ecl D:\ecl\a.ecl -Provider cuda -WithoutCuda -ReportDir $dir
"exit $LASTEXITCODE"
Test-Path $dir
```

- [ ] 標準エラーに `error: candidate provider unavailable: ...`（利用できない理由を含む）が出る。例: `... Error loading "...\onnxruntime_providers_cuda.dll" which depends on "cublasLt64_13.dll" which is missing.`
- [ ] 終了コード 1
- [ ] 解析が始まっていない（`$dir` が作られていない、または `report.json` / CSV がない）
- [ ] 参考: 同じ状態で `-Provider auto` にすると中止ではなく CPU に切り替わり、`provider cuda unavailable (...); trying next ...` が出る

### 4.4 CUDA Graph（実行グラフの再利用）の GPU での動作確認

CUDA Graph は GPU 実機でしか確認できません。本番で `cuda_graph` を有効にする前に必ず実施します。

```powershell
# CUDA の PATH 設定（README「NVIDIA（CUDA）」と同じ）
$env:CUDA_PATH = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.4"
$env:ORT_CUDA_VERSION = "13"
$env:PATH = "C:\Program Files\NVIDIA\CUDNN\v9.26\bin\13.4\x64;$env:CUDA_PATH\bin;$env:CUDA_PATH\bin\x64;$env:PATH"

# 端数あり（37 窓 = 16 + 16 + 5、最後は零埋め）
.\target\release\holter-analysis-assist.exe analyze-ecl D:\ecl\a.ecl `
  --model resources\models\phase2_rev1_dynamic.onnx `
  --provider cuda --batch-size 16 --cuda-graph on --max-windows 37 `
  --output output\cuda_graph_check\beat_results.csv
```

- [ ] 解析が成功し、`perf:` 行が `provider=cuda batch_size=16 model_batch=dynamic cuda_tuning=tf32=default,conv1d_pad_to_nc1d=default,cuda_graph=on` を含む
- [ ] 4.2 の C8 / C9（`compare-accel` での CUDA Graph）が成功し、精度が C4 / C6（CUDA Graph なし）と同程度
- [ ] 端数のある ECL 全体（丸 1 日）でも成功する
- [ ] 本番モデル（固定バッチ 1）でも `--cuda-graph on` で成功する
- [ ] 失敗した場合は、エラー文（`CUDA graph failed (...); set cuda_graph=false to disable CUDA graph`）と発生箇所（セッション生成時か、初回推論時か）を記録する

失敗時の挙動:

- `--provider cuda`（明示）: セッション生成時・初回推論時のどちらの失敗もエラーで終了します。
- `--provider auto`: CUDA のセッション生成時に CUDA Graph が使えないと分かった場合は CPU に切り替わり、標準エラーに `provider cuda unavailable (...); trying next ...` が出ます（その後 CPU では CUDA チューニングが適用されない旨の `warning:` も出ます）。一方、暖機・初回推論での捕捉失敗は `auto` でもエラーになります。
- 無効にするには `cuda_graph=false`（CLI は `--cuda-graph off`）を指定するか、項目自体を外します。

### 4.5 HTTP で CUDA Graph を使ったときの動作確認

HTTP では解析リクエストごとに別スレッドから同じ常駐モデル（同じ CUDA Graph）を再生します。この組合せは Windows 実機での確認が必要な既知のリスクです。

1. ini を用意する（`config/http.ini.example` と `config/license.ini.example` をマージ）:

   ```ini
   [http]
   bind=127.0.0.1:8080
   model_path=resources/models/phase2_rev1_dynamic.onnx
   provider=cuda
   batch_size=16
   cuda_graph=true

   [license]
   server_url=https://license.example.com
   ```

2. 4.4 と同じく CUDA の PATH を設定したシェルで起動する:

   ```powershell
   .\target\release\holter-http-api.exe --config C:\path\to\http-gpu.ini
   # holter-http-api.exe が起動できない場合（5 章の Smart App Control）は CLI から同じサーバーを起動できる
   .\target\release\holter-analysis-assist.exe serve-http --config C:\path\to\http-gpu.ini
   ```

3. 起動ログを確認する:
   - [ ] `holter-http-api: model ready provider=cuda batch_size=16 model_batch=dynamic cuda_tuning=tf32=default,conv1d_pad_to_nc1d=default,cuda_graph=on warmup_ms=... source=...` が出たあとに `holter-http-api: listening on http://127.0.0.1:8080` が出る
   - [ ] `warmup_ms` を記録する（暖機に CUDA Graph の捕捉が含まれる）
   - [ ] 暖機で失敗した場合はポートを開かずに終了し、失敗理由が表示される

4. 2 リクエストを連続で送る（PowerShell 5.1 では `curl` が別コマンドの別名になるため `curl.exe` を使う）:

   ```powershell
   $ecl = "D:\ecl\2501103675_20250512_1415_2359.ecl"
   $name = Split-Path $ecl -Leaf
   1..2 | ForEach-Object {
       curl.exe -sS -o "output\http_seq_$_.json" -w "%{http_code}`n" `
         -H "Accept: application/json" -F "ecl=@$ecl;filename=$name" `
         http://127.0.0.1:8080/v1/analyze
   }
   ```

   - [ ] どちらも `200`
   - [ ] サーバーのログにリクエストごとに `holter-http-api: perf: provider=cuda batch_size=16 ... response_encode_ms=...` が出る

5. 2 リクエストを同時に送る:

   ```powershell
   $jobs = 1..2 | ForEach-Object {
       $i = $_
       Start-Job -ScriptBlock {
           param($ecl, $name, $i, $cwd)
           Set-Location $cwd
           curl.exe -sS -o "output\http_par_$i.json" -w "%{http_code}" `
             -H "Accept: application/json" -F "ecl=@$ecl;filename=$name" `
             http://127.0.0.1:8080/v1/analyze
       } -ArgumentList $ecl, $name, $i, (Get-Location).Path
   }
   $jobs | Wait-Job | Receive-Job
   Get-FileHash output\http_seq_1.json, output\http_seq_2.json, output\http_par_1.json, output\http_par_2.json
   ```

   - [ ] どちらも `200`（順番待ちで処理され、どちらも失敗しない）
   - [ ] 連続・同時の 4 つの応答のハッシュが一致する
   - [ ] サーバーのプロセスが落ちていない（続けて `GET /health` が `200`）

## 5. 注意点

- **`--prob-stride` とメモリ**: 基準の解析中、`--prob-stride` ウィンドウごとに beat / event の出力（1 ウィンドウ約 160 KB）をメモリに保持します。`--prob-stride 1`（既定）で丸 1 日の ECL は約 800 MB、7 日分は約 5.7 GB です。複数日の ECL では `--prob-stride 10` など大きめの値にしてください（rhythm は間引かず全ウィンドウを比較します）。
- **出力先は使い回さない**: `report.json` / `report.md` / CSV は上書きされますが、前回の実行で作られた ECL ごとのフォルダは残ります。途中で失敗すると、新しい CSV の横に前回の `report.json` / `report.md` が残ることもあります。実行ごとに新しい `--report-dir` を使ってください（ラッパは既定でそうします）。
- **負の値**: 閾値・件数に負の値は使えません（引数エラー）。値を渡すときは `--max-prob-abs-diff=0.001` のように `--flag=値` の形で書くと、`-` で始まる値でも確実に値として解釈されます（ラッパは閾値をこの形で渡します）。
- **終了コード 2 の意味**: 閾値の不合格と、引数エラー（clap）の両方が `2` です。引数エラーは解析を始める前に終了し、レポートを書きません。引数エラーでは標準エラーに clap の `error: ...` 行と `Usage:` が出るので、それと `report.md` の「判定」で区別します。
- **固定バッチモデル**: 本番モデル（固定バッチ 1）では `batch_size` は警告付きで無視されます。まとめ推論の効果は可変バッチモデルで計測します（1.2）。
- **ライセンス計上**: `compare-accel` は ECL あたり 2 回（基準と候補）計上します。4.2 のように組合せを回すと計上回数が増えるので、計測に使う ECL 数を絞るか、`--max-windows` で試行してから本計測します（`--max-windows` でも計上回数は変わりません）。
- **Smart App Control（ソースからビルドする開発者向け）**: Windows 11 のスマート アプリ コントロールが有効な端末では、ビルドし直した未署名の exe が `os error 4551`（アプリケーション制御ポリシーによるブロック）で起動できないことがあります。`holter-http-api.exe` がブロックされる場合は `holter-analysis-assist.exe serve-http --config ...` で同じ HTTP サーバーを起動できます。CLI 自体がブロックされる場合は、Windows セキュリティの「アプリとブラウザー コントロール」の設定を端末の管理者と確認してください（設定の変更はこの手順書の範囲外です）。

## 6. 記録表

| 日付 | GPU / ドライバ | CUDA / cuDNN | ECL（窓数） | ケース | 候補 windows/s（推論） | speedup 推論 / 総 | 対応率 基準比 / 候補比 | 拍ラベル一致率 | リズム区間一致率 | 出力確率 最大差 | 位置ずれ最大 | GPU メモリ | 備考 |
|---|---|---|---|---|---|---|---|---|---|---|---|---|---|
| | | | | 4.1 CPU B16 | | | | | | | | — | |
| | | | | C1〜C9 | | | | | | | | | |
| | | | | 4.3 CUDA 不可 | — | — | — | — | — | — | — | — | 終了コード / エラー文 |
| | | | | 4.4 CUDA Graph | | | | | | | | | 成功 / 失敗箇所 |
| | | | | 4.5 HTTP | — | — | — | — | — | — | — | | warmup_ms / 連続・同時の結果 |

### 実測メモ（2026-09-28、速度のみ）

`compare-accel` による精度比較ではなく、`analyze-ecl` の `perf:` 行から推論段階の速度だけを拾ったものです。

- 端末: GeForce RTX 4080（ドライバ 610.88）、CUDA 13.4 / cuDNN 9.26
- モデル: `phase2_rev1_dynamic.onnx`（`export_onnx.py --dynamic-batch --verify-batch 4` で再エクスポート。Keras との最大差 7.5e-5、バッチ 16 とバッチ 1 の最大差 2.4e-7）
- ECL: 丸 1 日（5,082 窓）の先頭 1,008 窓、`--provider cuda`、チューニングは未指定（ONNX Runtime 既定）

| batch_size | 推論 ms | windows/s（推論） | batch 1 比 |
|---|---|---|---|
| 1 | 31,459 | 32.0 | 1.0 |
| 2 | 18,808 | 53.6 | 1.7 |
| 3 | 15,886 | 63.5 | 2.0 |
| 4 | 12,743 | 79.1 | 2.5 |
| 8 | 10,935 | 92.2 | 2.9 |
| 16 | 10,915 | 92.4 | 2.9 |
| 32 | 12,066 | 83.5 | 2.6 |

- 8 で頭打ちになり、32 では低下しました。推論中の GPU 使用率は 98%（240 W）で、演算そのものが律速です（1 窓あたり畳み込みだけで約 115 GFLOP、ONNX ノード 3,534 個）。
- TF32 は ONNX Runtime 既定で有効です。`--cuda-tf32 false` にすると batch 16 で 79.4 windows/s に下がりました。
- HTTP（丸 1 日 5,082 窓）での結果: batch 4 は推論 65.1 秒（78.1 windows/s）、batch 16 は 55.0〜56.3 秒（90〜92 windows/s）でした。batch 16 に `cuda_tf32=true` と `cuda_graph=true` を加えると 54.5 秒（93.3 windows/s）で、差はありませんでした。前処理・後処理・出力は合計約 2.7 秒です。
- Windows 版 TensorFlow で `--verify-batch 16` を指定すると、Keras 側の推論中に Python が異常終了します（0xC0000005）。ONNX 自体は正常なので、この端末では `--verify-batch 4`（既定）で検証してください。

## 範囲外

- 配布物（Windows インストーラ・Docker イメージ・Linux パッケージ）への新たなランタイムライブラリの同梱（CUDA Toolkit / cuDNN は利用者の環境に導入する）
- TensorRT 実行プロバイダ
- FP16 / INT8 など FP32 以外の演算精度（本機能は FP32 のみ）
- 複数 GPU での分散実行（GPU は 0 番固定）
- チューニング設定の採否閾値（合否基準）の確定（本手順の計測結果を見て別途決める）
