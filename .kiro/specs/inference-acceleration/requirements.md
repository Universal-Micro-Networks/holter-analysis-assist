# Requirements Document

## Project Description (Input)

- **誰が困っているか**: Windows の NVIDIA GPU（Ampere 世代以降）端末でホルター ECL を解析するオペレータ、および HTTP API / UI コンソール経由で解析を依頼する連携システム。
- **現状**: CUDA 実行時も 20 秒ウィンドウを 1 件ずつ推論しており（丸 1 日 ECL で約 2,000 回の推論呼出し）、CUDA 実行設定は既定値のまま、HTTP API はリクエストごとにモデルを読み込み直している。どの段階に時間がかかっているかも計測されていない。
- **何を変えたいか**: 段階別の計測を入れたうえで、複数ウィンドウのまとめ処理、HTTP でのモデル常駐と暖機、CUDA 実行チューニングを導入し、GPU 実行を高速化する。チューニング設定によって結果が変わりうるため、CPU FP32 を基準とした精度・速度の比較レポートを作れるようにし、採否の閾値は計測結果を見て決める。TensorRT 実行および FP16 精度モードは今回のスコープに含めない。

## Introduction

本仕様は、Holter Analysis Assist の ONNX 推論を GPU 環境で高速化するための機能群を定義する。対象は、段階別処理時間の計測、複数ウィンドウのまとめ処理、HTTP サービスでのモデル常駐と暖機、CUDA 実行時のチューニング設定、および CPU FP32 を基準とした精度・速度比較レポートである。解析結果の出力契約（列・ラベル体系・形式）とライセンス計上単位（1 解析ジョブ = 1 回）は変更しない。チューニング設定の採否閾値は本仕様では固定せず、比較レポートの計測結果に基づいて別途決定する。

## Boundary Context

- **In scope**:
  - 解析ジョブの段階別処理時間と使用実行プロバイダ・まとめ処理件数の診断出力
  - 1 回の推論呼出しで複数ウィンドウを処理するまとめ処理と、その件数の設定
  - HTTP サービス起動時のモデル読込み・暖機と、リクエスト間でのモデル再利用
  - CUDA 実行時のチューニング項目の設定
  - 同一 ECL に対する基準設定（CPU・FP32）と候補設定の精度・速度比較レポート
  - 上記設定の CLI オプション / ini 設定と、README・設定例での文書化
  - Windows GPU 端末での計測手順の文書化
- **Out of scope**:
  - TensorRT 実行プロバイダ（エンジン構築・エンジンキャッシュを含む）
  - FP16 / INT8 など FP32 以外の演算精度モード、およびそれらに変換したモデルの配布
  - チューニング設定の採否閾値（合否基準）の確定
  - 複数 GPU への分散実行
  - モデルの再学習・構造変更
  - 解析結果の出力契約（列・ラベル体系・CSV / JSON 形式）の変更
  - ライセンス確認・計上単位の変更
- **Adjacent expectations**:
  - 既存の実行プロバイダ選択（`auto` = CUDA → CPU、`cuda`、`cpu`）の意味と選択肢は変更しない
  - `model-embedding`: 配布ビルドの埋め込みモデルでも本仕様の機能（まとめ処理を含む）が外部の生モデルファイルなしに動作すること。モデルの出し直しは既存の埋め込み経路で配布する
  - `license-client`: 1 解析ジョブ = 1 回の許可確認・計上を維持する。本仕様はゲートを直接呼ばない
  - `http-api`: `[http]` の既存キー意味を変えずに設定項目を追加する。起動時ライセンス確認・サイズ上限・タイムアウトの契約は変更しない
  - `packaging-distribution`: 既存 CPU 配布成果物は GPU ライブラリなしで従来どおり動作すること。配布物に新たなランタイムを同梱しない

## Requirements

### Requirement 1: 段階別処理時間の計測

**Objective:** As a 解析オペレータ, I want 解析ジョブのどの段階に時間がかかっているかを確認できる, so that 高速化の効果とボトルネックを数値で判断できる

#### Acceptance Criteria

1. When 解析ジョブが完了する, the Holter Analysis Assist shall 前処理・推論・後処理・出力生成の各段階の所要時間、総所要時間、処理ウィンドウ数を診断出力に記録する
2. When 解析ジョブが完了する, the Holter Analysis Assist shall 実際に使用した実行プロバイダ、まとめ処理件数、有効な CUDA チューニング項目を診断出力に記録する
3. The Holter Analysis Assist shall 段階別計測の出力を解析結果本体（CSV / JSON）に混在させない
4. When HTTP サービスが解析リクエストの処理を完了する, the Holter Analysis Assist HTTP Service shall 同じ段階別計測をサーバーログに記録する

### Requirement 2: 複数ウィンドウのまとめ処理

**Objective:** As a 解析オペレータ, I want 複数の 20 秒ウィンドウを 1 回の推論でまとめて処理できる, so that GPU の処理能力を活かして丸 1 日の解析時間を短縮できる

#### Acceptance Criteria

1. The Holter Analysis Assist shall 1 回の推論呼出しで処理するウィンドウ数（まとめ処理件数）を設定で指定できる
2. Where まとめ処理件数が指定されない, the Holter Analysis Assist shall 文書化された既定のまとめ処理件数を用いる
3. When 総ウィンドウ数がまとめ処理件数の倍数でない, the Holter Analysis Assist shall すべてのウィンドウを欠落・重複なく処理し、端数分のウィンドウの結果も解析結果に含める
4. The Holter Analysis Assist shall まとめ処理件数の値によって、解析結果の出力列・形式・ウィンドウの処理順序を変えない
5. When CPU・FP32 でまとめ処理件数 1 と既定のまとめ処理件数の結果を同一 ECL で比較する, the Holter Analysis Assist shall 拍の位置・拍ラベル・リズム区間が一致する解析結果を出力する
6. If まとめ処理件数に 1 未満または文書化された上限を超える値が指定される, the Holter Analysis Assist shall 解析を開始せず、設定エラーとして識別可能に報告する
7. Where 埋め込みモデルを含む配布ビルドである, the Holter Analysis Assist shall 外部の生モデルファイルなしにまとめ処理を利用できる

### Requirement 3: HTTP サービスでのモデル常駐と暖機

**Objective:** As a 連携システム運用者, I want HTTP サービスがモデルを起動時に準備し使い回す, so that 解析リクエストごとのモデル読込み・GPU 初期化の待ち時間をなくせる

#### Acceptance Criteria

1. When HTTP サービスが起動する, the Holter Analysis Assist HTTP Service shall 推論モデルの読込みと暖機推論を完了してからリクエスト受付を開始する
2. While HTTP サービスが稼働している, the Holter Analysis Assist HTTP Service shall 解析リクエストごとにモデルを読み込み直さず、起動時に準備したモデルを再利用する
3. If 起動時のモデル読込みまたは暖機推論が失敗する, the Holter Analysis Assist HTTP Service shall リッスンを開始せず、失敗理由を識別可能な形で示して終了する
4. When 複数の解析リクエストが同時に到着する, the Holter Analysis Assist HTTP Service shall モデル共有を理由にいずれのリクエストも失敗させず、順番待ちまたは並行処理によってそれぞれ完了させる
5. When HTTP サービスがリクエスト受付を開始する, the Holter Analysis Assist HTTP Service shall 準備したモデルの実行プロバイダ・まとめ処理件数・暖機に要した時間をログに示す
6. The Holter Analysis Assist HTTP Service shall モデル常駐化によって、1 解析リクエストあたり 1 回のライセンス確認・計上という単位を変えない

### Requirement 4: CUDA 実行時のチューニング設定

**Objective:** As a 解析オペレータ, I want CUDA 実行時のチューニング項目を切り替えられる, so that 自分の GPU 環境で速度と精度のバランスを調整できる

#### Acceptance Criteria

1. Where 実行プロバイダが CUDA である, the Holter Analysis Assist shall TF32 演算の許可、1 次元畳み込み向け最適化、実行グラフの再利用の各項目を設定で個別に切り替えられる
2. Where CUDA チューニング項目が指定されない, the Holter Analysis Assist shall 本機能導入前の CUDA 実行と同等の挙動となる文書化された既定値を用いる
3. If CUDA 以外の実行プロバイダで CUDA 専用のチューニング項目が指定される, the Holter Analysis Assist shall 当該項目を適用せず、適用しなかったことを警告として診断出力に示したうえで解析を継続する
4. If CUDA チューニング項目に不正な値が指定される, the Holter Analysis Assist shall 解析を開始せず（HTTP サービスは起動せず）、設定エラーとして識別可能に報告する

### Requirement 5: 精度・速度比較レポート

**Objective:** As a 品質責任者, I want 同一 ECL を基準設定と候補設定で解析した差分と速度をレポートで確認できる, so that まとめ処理や CUDA チューニング設定を採用してよいかを計測結果に基づいて判断できる

#### Acceptance Criteria

1. When 開発者が 1 つ以上の ECL と候補設定（実行プロバイダ・まとめ処理件数・CUDA チューニング項目）を指定して比較を実行する, the Accuracy Comparison Tool shall 基準設定（CPU・FP32・まとめ処理件数 1）と候補設定の解析結果の差分をレポートとして出力する
2. The Accuracy Comparison Tool shall レポートに、リズム区間の一致率、検出拍数の差、指定した許容サンプル幅内で対応付けられた拍の割合と位置ずれ（最大・平均）、対応拍のラベル一致率とラベル混同表、ウィンドウ単位の出力確率の最大絶対差を含める
3. The Accuracy Comparison Tool shall レポートに、基準設定と候補設定それぞれの段階別所要時間、総所要時間、1 秒あたりの処理ウィンドウ数を含める
4. When 複数の ECL が指定される, the Accuracy Comparison Tool shall ファイル別の結果と全体の集計結果を出力する
5. The Accuracy Comparison Tool shall レポートを機械可読な形式と、人が読める要約の両方で出力する
6. The Accuracy Comparison Tool shall 合否の閾値を固定せず、閾値が指定された場合に限り各指標の合否を判定してレポートに示す
7. If 候補設定の実行プロバイダが実行環境で利用できない, the Accuracy Comparison Tool shall 比較を中止し、利用できない理由を示す

### Requirement 6: 設定と互換性

**Objective:** As a 解析オペレータ, I want 新しい設定を従来の設定方法の延長で指定でき、指定しなければ従来どおり動く, so that 既存の運用や連携を壊さずに高速化を試せる

#### Acceptance Criteria

1. The Holter Analysis Assist shall 本仕様の設定項目を、CLI ではコマンドラインオプションで、HTTP サービスでは既存の ini 設定ファイルで、同じ意味・同じ値の体系で指定できる
2. Where 本仕様の設定項目がいずれも指定されない, the Holter Analysis Assist shall 本機能導入前と同一の解析結果契約（出力列・ラベル体系・形式）を保つ
3. The Holter Analysis Assist shall 既存の実行プロバイダ指定値（`auto` / `cuda` / `cpu`）の意味と選択肢を変えない
4. The Holter Analysis Assist shall Windows と Linux で同一の設定項目名を用いる
5. The Holter Analysis Assist shall 本仕様の設定項目、既定値、有効な組み合わせを設定例ファイルと README に記載する

### Requirement 7: 配布と文書

**Objective:** As a リリース担当者, I want 配布物を増やさずに GPU 高速化を案内できる, so that 既存の配布形態のまま利用者が高速化設定を試せる

#### Acceptance Criteria

1. The Holter Analysis Assist shall 本機能のために Windows インストーラ、Docker イメージ、Linux パッケージへ新たなランタイムライブラリを同梱しない
2. While GPU が利用できない, the Holter Analysis Assist shall 本仕様の設定項目を指定しない限り従来どおり起動・解析できる
3. The Holter Analysis Assist shall Windows GPU 端末で比較レポートを用いて速度・精度を計測する手順を文書に記載する

### Requirement 8: 範囲外の明確化

**Objective:** As a ステークホルダー, I want 本仕様が担わない責務が明確である, so that TensorRT 対応や精度基準の確定を本仕様に期待しない

#### Acceptance Criteria

1. The Holter Analysis Assist shall TensorRT 実行プロバイダへの対応を本機能の成果に含めない
2. The Holter Analysis Assist shall FP16 / INT8 など FP32 以外の演算精度モード、およびそれらに変換したモデルの配布を本機能の成果に含めない
3. The Holter Analysis Assist shall チューニング設定の採否閾値（合否基準）の確定を本機能の成果に含めない
4. The Holter Analysis Assist shall 複数 GPU への分散実行を本機能の成果に含めない
5. The Holter Analysis Assist shall 解析結果の出力契約およびライセンス確認・計上単位の変更を本機能の成果に含めない
