# 引き継ぎメモ: inference-acceleration（Mac → Windows）

作成: 2026-09-27 / 作成環境: macOS（GPU なし）

## 1. 現在地

`/kiro-impl inference-acceleration` を自律モード（タスクごとに実装 → 独立レビュー → 検証 → `tasks.md` に `[x]` → コミット）で進めている途中です。

### push 済み（`main`）

| コミット | 内容 |
|---|---|
| `e46ee92` | spec（requirements / design / tasks）承認版 |
| `32605e4` | 既存ソースへの `cargo fmt` 適用のみ（作業前から `cargo fmt --check` が 24 ファイルで失敗していたため） |
| `a9582f8` | 1.1 推論設定の型（`src/inference_options.rs`） |
| `b6409ab` | 1.4 `export_onnx.py --dynamic-batch` / `--verify-batch N` |
| `8d9bf5a` | 1.3 極小 ONNX フィクスチャ（`tests/fixtures/`）と `http_api_listen` テストの修正 |
| `213f2d4` | 1.2 `perf` / `accel_compare` の空骨格 |
| `84ed77c` | 2.1 BatchPlan（`src/phase2/batch_plan.rs`、`ModelBatchShape`） |
| `246c486` | 2.6 比較用の精度指標（`src/accel_compare/metrics.rs`） |
| `6440ac5` | 2.2 設定付きモデル読込み・CUDA チューニング・実効設定（`src/phase2.rs`） |

`tasks.md` 上は 1.1〜1.4、2.1、2.2、2.6 が `[x]`（20 タスク中 7 つ）。Mac 側の作業はここで止めており、未コミット・未 push の変更はありません。**Windows では `git pull` 後、次は 2.3 から再開します**（2.5 も 2.2 完了で着手可能）。

## 2. Windows 環境の準備

1. `git pull`（`main`）
2. Rust stable（rustup）。Mac ではリポ内の `.cargo-tools` / `.rustup-tools` を使っていたが、Windows では通常の `%USERPROFILE%\.cargo` で問題ない。
3. Visual Studio 2022（C++ ビルドツール）。`tools/compare/run_cuda_bench.ps1` は `VsDevCmd.bat` 経由でビルドしている。
4. CUDA / cuDNN（README の「NVIDIA（CUDA）」節と同じ）:

```powershell
$env:CUDA_PATH = "C:\Program Files\NVIDIA GPU Computing Toolkit\CUDA\v13.4"
$env:ORT_CUDA_VERSION = "13"
$env:PATH = "C:\Program Files\NVIDIA\CUDNN\v9.26\bin\13.4\x64;$env:CUDA_PATH\bin;$env:CUDA_PATH\bin\x64;$env:PATH"
```

5. Git 管理外のリソースを配置:
   - `resources/models/phase2_rev1.onnx`（本番モデル。固定バッチ 1）
   - `resources/samples/sample.ecl`（実 ECL を使うテスト・比較用。無ければ該当テストは skip）
6. テスト用の極小 ONNX（`tests/fixtures/phase2_tiny_dynamic.onnx` / `phase2_tiny_fixed1.onnx`）は Git 管理下なので追加作業は不要。再生成する場合のみ Python の `onnx` と `numpy` が必要（`tests/fixtures/README.md`）。

## 3. 作業の再開方法

```text
/kiro-impl inference-acceleration
```

タスク番号を省略すると、`tasks.md` の未完了タスクから自律モードで再開します。残りと依存関係:

| タスク | 内容 | 依存 |
|---|---|---|
| 2.3 | まとめ推論 `infer_batch` と暖機 `warm_up` | 2.2 |
| 2.4 | CUDA Graph 用の固定バッファ実行経路 | 2.2, 2.3 |
| 2.5 | 段階別計測と診断ログ（`src/perf.rs`） | 1.2, 2.2 |
| 2.7 | 比較レポートと閾値判定 | 2.5, 2.6 |
| 3.1 | 解析パイプラインのまとめ推論・計測対応 | 2.3, 2.5 |
| 3.2〜3.5 | HTTP 設定キー → CLI オプション → HTTP 暖機 → HTTP ハンドラ | 3.1 以降 |
| 3.6, 3.7 | 比較の実行処理と `compare-accel` サブコマンド | 2.6, 2.7, 3.1, 3.3 |
| 4.1, 4.2 | 文書・計測手順書・ラッパ、全体回帰確認 | — |

最後に `/kiro-validate-impl inference-acceleration` で GO / NO-GO を判定します。

## 4. 検証コマンド（CI と同じ）

```powershell
cargo fmt --all -- --check
cargo clippy --all-targets -- -D warnings
cargo test
cargo check --no-default-features   # cuda 機能なしでもビルドできること（リリースビルドは --no-default-features --features embedded-model）
```

作業開始時点のベースライン（Mac）: `cargo test` 全件成功、clippy・fmt チェック成功。

## 5. 注意点（これまでに踏んだもの）

- **MSRV は 1.74**（`Cargo.toml` の `rust-version`）。clippy の `incompatible_msrv` で、`Option::is_none_or`（1.82）など新しい std API は `-D warnings` で落ちる。
- **`cargo fmt --check` は CI で必須**。以前のコミットで崩れていたので `32605e4` で揃えた。コミット前に必ず確認する。
- **`tests/http_api_listen.rs`** は HTTP サーバーを実際に起動してポートを使う。以前は存在しないモデルパスを指していて 5 件失敗していたが、既定モデルを `tests/fixtures/phase2_tiny_dynamic.onnx` に変えて解消済み。
- **cuda 機能の cfg 分岐**: CUDA EP 固有の API（`with_tf32` / `with_conv1d_pad_to_nc1d` / `with_cuda_graph`、IoBinding の CUDA アロケータ）は `#[cfg(feature = "cuda")]` で囲み、`--no-default-features` でもビルドできるようにする。
- **ライセンス計上**: 正本入口 `analyze_ecl_with_source` の中でジョブあたり 1 回。新しく計上する公開入口を増やさない。比較サブコマンドは ECL あたり 2 回（基準と候補）計上する設計。

## 6. レビューからの申し送り（後続タスクで必ず反映）

- **2.4（最重要）**: 2.2 で `cuda_graph=Some(true)` のとき CUDA EP に `with_cuda_graph(true)` を渡すようにしたが、固定アドレスで実行する経路（IoBinding の CudaGraphRunner）はまだ無い。現状 `cuda_graph_active=true` でも通常の `session.run` を通るため、GPU では失敗または誤った結果になり得る。**2.4 で `cuda_graph_active` のとき CudaGraphRunner 経由に切り替え、常に `[batch_size,10000,1]` の形状で実行することを、3.2 / 3.3（CLI・ini から cuda_graph を指定可能にする）より前に完了させる。** 現時点では利用者が cuda_graph を指定できる経路は無い。
- **2.7**: 2.6 の指標は分母 0 の率を 1.0（比較対象なし＝一致）としている。基準に拍があり候補が 0 拍のとき、`match_rate_vs_baseline` は 0.0 になるので異常は隠れないが、`match_rate_vs_candidate` や対応拍ベースの一致率は 1.0、位置ずれは 0 になる。閾値 `min_beat_match_rate` は `match_rate_vs_baseline`（または 2 つの率の小さい方）で判定し、人向け要約では `matched_beats=0` のときの 1.0 が「比較対象なし」だと分かるように表示する。最大差の閾値は `actual <= max` の形で書き、NaN（JSON では null）が不合格になるようにする。
- **3.6**: 2.6 で追加した `MetricsError`（`beat_time` の解析失敗）を受ける変種が design の `CompareError` に無い。3.6 で変種を足すか変換方法を決める。
- **2.1**: 固定バッチモデルで要求値が無視されたときの警告文は英語（`model has fixed batch {n}; batch_size={requested} is ignored`）。2.2 以降の警告は stderr に `warning: {note}` の書式で出る。他の警告文と言語を揃える。
- **2.2**: `cuda_applied` は解決後のプロバイダが CUDA なら（チューニング未指定でも）true。ログの `cuda_tuning=` 表示は `CudaTuning::describe()` から作る。

## 7. Windows GPU 端末で確認が必要なこと

Mac（GPU なし）では確認できない項目です。

**今の時点でできること**
- `git pull` 後に `cargo test` / clippy / fmt が Windows でも通ること。
- TensorFlow 環境があれば、可変バッチモデルの再エクスポート（`tools/export/README.md` の「可変バッチでの再エクスポート」節）:
  `python tools/export/export_onnx.py --dynamic-batch --verify-batch 4`
  Keras と ONNX Runtime の一致、バッチ N とバッチ 1 の一致がこのとき検証される（Mac では未実行）。

**全タスク完了後（4.1 で手順書 `docs/perf/windows-gpu-benchmark.md` と `tools/compare/run_accel_compare.ps1` を作成予定）**
- CPU でまとめ件数 1 と既定 16 の結果が一致すること。
- CUDA のチューニング 3 項目（`cuda_tf32` / `cuda_conv1d_pad_to_nc1d` / `cuda_graph`）の組合せごとの速度と精度（`compare-accel` で CPU・FP32・まとめ 1 と比較）。
- CUDA が使えない状態で候補に CUDA を指定すると比較が中止されること。
- **CUDA Graph を HTTP で使ったときの動作**: リクエストごとに `spawn_blocking` の別スレッドから同じグラフを再生するので、Windows 実機での検証が必要（設計上の既知リスク）。
- HTTP 起動時の準備ログ（`holter-http-api: model ready provider=… batch_size=… warmup_ms=…`）と、解析ごとの計測ログ（`perf: …`）が標準エラーに出ること。
