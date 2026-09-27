# Research & Design Decisions

## Summary
- **Feature**: `inference-acceleration`
- **Discovery Scope**: Extension（既存 Phase-2 推論・解析パイプライン・HTTP サービスへの拡張。light discovery）
- **Key Findings**:
  - 配布中の `phase2_rev1.onnx` は入力 `[1, 10000, 1]` の **固定バッチ 1**（`tools/export/export_onnx.py --batch-size` で焼き込み）。まとめ処理にはモデルの再エクスポート（可変バッチ）が必要で、Rust 側はモデルの入力形状を読んで実効まとめ件数を決める必要がある。
  - HTTP のモデル常駐は既にコミット `ae3c9e6` で実装済み（`SharedModel::Resident(Arc<Mutex<Phase2Model>>)` + `analyze_ecl_with_model`）。同時リクエストは Mutex で順番待ちになる。**暖機・常駐時のログ（まとめ件数・暖機時間）は未実装**。
  - ONNX Runtime CUDA EP の `use_tf32` は **既定 1（有効）**（ORT 1.18+）。`ort` クレートの doc コメント「disabled by default」は誤り。したがって「未指定 = ORT に何も渡さない」三値設定にすることで、導入前の挙動（Req 4.2）を保てる。

## Research Log

### 既存推論パスとモデル I/O
- **Context**: まとめ処理の実現可否。
- **Sources Consulted**: `src/phase2.rs`, `resources/models/phase2_rev1.onnx.json`, `tools/export/export_onnx.py`, `strings` によるモデル内 dim_param 探索。
- **Findings**:
  - `Phase2Model::infer_window` は `Array3::(1, 10000, 1)` を毎回確保し `session.run` → `beat`/`event`/`rhythm` を抽出。
  - モデル JSON は `shape: [1, 10000, 1]`、ONNX 内に `unk__` 等の動的次元名なし → 固定バッチ 1。
  - エクスポートは `tf.TensorSpec((batch_size, 10000, 1))`。`None` にすれば可変バッチで出力できる（tf2onnx 標準）。
  - `ort` rc.13: `session.inputs()[0].dtype().tensor_shape()` で次元取得可能（動的次元は負値）。
- **Implications**: Rust 側で `ModelBatchShape { Dynamic, Fixed(n) }` を判定し、固定 1 の旧モデルでも動作（実効 1 件、警告）。可変バッチモデルは `--dynamic-batch` で再エクスポート。

### CUDA EP オプション（ort rc.13 / ORT 公式）
- **Context**: Req 4 のチューニング項目と既定値。
- **Sources Consulted**: `ort-2.0.0-rc.13/src/ep/cuda.rs`、[ORT CUDA EP docs](https://onnxruntime.ai/docs/execution-providers/CUDA-ExecutionProvider.html)。
- **Findings**:
  - `with_tf32(bool)` → `use_tf32`（ORT 既定 1）。
  - `with_conv1d_pad_to_nc1d(bool)` → `cudnn_conv1d_pad_to_nc1d`（既定 0）。3 次元入力の Conv（Conv1D）に効き、A100 等で大きく改善しうる。結果は変わらない。
  - `with_cuda_graph(bool)` → `enable_cuda_graph`（既定 0）。制約: 制御フロー演算子不可、**入出力形状固定**、**入出力アドレス固定のため IoBinding 必須**、全ノードが CUDA EP 上であること、同時 Run 非対応。
  - `cudnn_conv_algo_search` 既定 EXHAUSTIVE（固定形状では既に最良。今回は項目化しない）。
- **Implications**: 項目は `Option<bool>` の三値。`None` は ORT へ未設定＝導入前と同一。CUDA Graph は専用の IoBinding 実行経路とテール詰め（padding）が必要。

### IoBinding / デバイスコピー（ort rc.13）
- **Context**: CUDA Graph 経路の実装可能性。
- **Sources Consulted**: `ort-2.0.0-rc.13/src/session/io_binding.rs`, `src/value/impl_tensor/copy.rs`, `src/memory.rs`。
- **Findings**: `session.create_binding()`, `bind_input`, `bind_output`, `run_binding`、`Allocator::new(&session, MemoryInfo::new(AllocationDevice::CUDA, 0, AllocatorType::Device, MemoryType::Default))`、`Tensor::copy_into`（ホスト→デバイス）、`Tensor::to(AllocationDevice::CPU, 0)` が利用可能。
- **Implications**: 固定デバイスバッファ（入力 `[B,10000,1]`、出力 3 本）を確保し、毎バッチでホスト→デバイスコピー → `run_binding` → デバイス→ホスト取得、で CUDA Graph の前提を満たせる。

### HTTP 常駐モデルの現状
- **Context**: Req 3 の差分特定。
- **Sources Consulted**: `src/http/state.rs`, `src/http/startup.rs`, `src/http/handlers/analyze.rs`, `git log`（`ae3c9e6`, `53f864a`）。
- **Findings**: 起動時 `AppState::from_config` でロード（失敗は `StartupError::Model` → リッスンしない）。リクエストは `spawn_blocking` 内で Mutex ロック → `analyze_ecl_with_model`。リクエスト単位の provider 上書きは一時セッションを別ロード。暖機・準備ログは provider/source のみ。
- **Implications**: 追加は「ロード後の `warm_up` と準備ログ」「設定（まとめ件数・チューニング）の受け渡し」「perf ログ」に限定。

### 出力契約への影響
- **Context**: Req 1.3 / 6.2（出力契約不変）。
- **Sources Consulted**: `src/http/response.rs`, `src/analyze.rs`。
- **Findings**: JSON は `AnalyzeSummaryJson`（beats / unknown_ones / short_run_ones / windows）へ明示マッピング。`AnalyzeSummary` にフィールドを追加しても JSON は変わらない。CSV は `BeatResultRow` の serialize。
- **Implications**: 計測結果は `AnalyzeSummary.perf` に載せ、CSV/JSON には出さない。

### テスト基盤
- **Context**: CI（GPU なし・実モデルなし）で検証可能な範囲。
- **Sources Consulted**: `src/phase2.rs` tests（実モデル不在時 skip）、`tools/export/make_smoke_onnx.py`、`.gitignore`（`*.onnx` 除外）。
- **Findings**: 実モデル依存テストは CI で skip。smoke ONNX も固定バッチ 1・未コミット。
- **Implications**: 可変バッチ／固定バッチ 1 の **極小フィクスチャ ONNX**（数 KB、要素ごとの Sigmoid 等で入力依存の決定的出力）を `tests/fixtures/` にコミットし、まとめ処理の順序・端数・一致を CI で検証する。GPU 依存項目は Windows 手動計測手順で検証。

### ライセンス計上と比較ツール
- **Context**: Req 5（比較）と「1 解析ジョブ = 1 計上」の両立、並列 gated 入口の禁止（http-api 仕様）。
- **Findings**: 正本入口は `analyze_ecl_with_source`（+ 常駐用 `analyze_ecl_with_model`）。`Phase2Model::infer_window` 自体は非ゲート（計上単位はジョブ）。
- **Implications**: 比較ツールは CLI サブコマンドとして CLI 起動時ライセンス確認を経て、各解析を既存ゲート経由で実行（ECL 1 件につき基準・候補で 2 計上）。ウィンドウ出力の取得は crate 内部限定（`pub(crate)`）の観測フックで行い、公開 gated 入口を増やさない。

## Architecture Pattern Evaluation

| Option | Description | Strengths | Risks / Limitations | Notes |
|--------|-------------|-----------|---------------------|-------|
| A: Phase2Model 拡張（採用） | まとめ処理・チューニング・暖機を `Phase2Model` に集約し、パイプラインはバッチ単位で呼ぶ | 既存の EP 解決・モデルソース経路を再利用。呼出し側の変更が小さい | `phase2.rs` の肥大化 → サブモジュール分割で緩和 | steering「推論差し替えは内側に閉じる」に合致 |
| B: 推論ワーカースレッド | 専用スレッドがモデルを所有しチャネルでバッチ受付 | CUDA Graph のスレッド親和性に強い | 新しい並行制御層。現行 Mutex 常駐と重複 | CUDA Graph が別スレッド再生で不安定な場合の代替案として保留 |
| C: 比較ツールを Python で実装 | 既存 `tools/compare` 同様 Python | 分析ライブラリが豊富 | 埋め込みモデル配布ビルドでは生 ONNX が無く比較不可。Rust 推論設定を再現できない | 不採用 |

## Design Decisions

### Decision: モデル入力形状に基づく実効まとめ件数
- **Context**: 既存モデルは固定バッチ 1、再エクスポート後は可変バッチ。
- **Alternatives Considered**:
  1. 可変バッチモデルを必須にする — 旧モデルで起動不能になる
  2. 入力形状を読んで実効件数を決める — 旧モデルも動く
- **Selected Approach**: 2。`Dynamic` → 設定値、`Fixed(n)` → n（設定値と異なれば警告、端数は詰め物で埋める）。
- **Rationale**: 後方互換（Req 6.2）と段階導入。
- **Trade-offs**: 旧モデルでは高速化されないが、警告で気づける。
- **Follow-up**: 再エクスポート時に Keras と ORT（バッチ N）の数値一致を確認。

### Decision: 三値のチューニング設定
- **Context**: Req 4.2（未指定なら導入前と同等）。
- **Selected Approach**: `Option<bool>`。`None` は ORT に渡さない。
- **Rationale**: ORT 既定（TF32 有効など）をそのまま維持でき、ort クレート doc と ORT 実装の食い違いにも影響されない。

### Decision: CUDA Graph は IoBinding 専用経路＋固定形状
- **Context**: CUDA Graph の前提（アドレス・形状固定）。
- **Selected Approach**: 有効時のみ固定デバイスバッファ + `run_binding`。全バッチを実効件数に詰め物で揃える。暖機で捕捉（capture）を完了させ、失敗はロード失敗として報告。
- **Trade-offs**: 端数バッチで無駄計算が出る（最大 B-1 ウィンドウ）。
- **Follow-up**: HTTP の `spawn_blocking` スレッドが毎回異なる点を Windows で検証（不安定なら Option B を別途検討）。

### Decision: 比較ツールは CLI サブコマンド + 観測フック
- **Context**: 埋め込みモデル配布ビルドでも比較可能にする／公開 gated 入口を増やさない。
- **Selected Approach**: `holter-analysis-assist compare-accel`。基準・候補のモデルを各 1 回ロードし、各 ECL を既存ゲート経由で 2 回解析。基準実行時に crate 内部フックでウィンドウ出力を保持し、候補実行時に逐次差分を取る（追加推論なし）。
- **Trade-offs**: 基準のウィンドウ出力保持にメモリ（丸 1 日 ≈ 330 MB）。`--prob-stride` で間引き可能。

### Generalization / Build vs Adopt / Simplification
- **Generalization**: `InferenceOptions`（provider + まとめ件数 + CUDA チューニング）を CLI・HTTP・比較ツール共通の単一型にし、同じパーサで値体系を統一（Req 6.1）。
- **Build vs Adopt**: 計測は `std::time::Instant` で十分（新依存なし）。比較指標（拍対応付け等）は小規模な純関数で自作（既存 `compare_pipelines.py` の ±80 ms 基準を踏襲）。
- **Simplification**: 推論ワーカースレッド・非同期パイプライン化・前処理並列化は今回入れない（計測結果を見て別途）。IoBinding は CUDA Graph 有効時のみ使用。

## Risks & Mitigations
- 可変バッチ再エクスポートでモデル内部に固定バッチ前提の Reshape がある — エクスポート時の Keras/ORT 比較（バッチ N）で検出、失敗時は固定バッチ N で出力（Rust 側は `Fixed(n)` を扱える）。
- CUDA Graph 捕捉失敗（CPU フォールバックノードの存在など） — 暖機で検出しロード失敗として明示、ドキュメントで `cuda_graph=false` を案内。
- まとめ件数過大による GPU メモリ不足 — 上限 256、既定 16、暖機（HTTP）で早期に失敗させる。
- TF32 有効（ORT 既定）で CPU FP32 と微差 — 比較レポートで定量化し、必要なら `cuda_tf32=false` を案内。
- 比較ツールのメモリ — `--prob-stride` で間引き。

## References
- [ONNX Runtime CUDA Execution Provider](https://onnxruntime.ai/docs/execution-providers/CUDA-ExecutionProvider.html) — `use_tf32` 既定 1、`cudnn_conv1d_pad_to_nc1d`、CUDA Graph の制約
- `ort` 2.0.0-rc.13 ソース（`src/ep/cuda.rs`, `src/session/io_binding.rs`, `src/value/impl_tensor/copy.rs`）
- `tools/compare/compare_pipelines.py` — 拍位置 ±80 ms 対応付けの既存基準
