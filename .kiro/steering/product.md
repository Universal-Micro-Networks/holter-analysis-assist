# Product Overview

検査会社のホルター心電図解析業務を支援する、不整脈リズム AI アプリケーション。  
心電図（ECL）を入力し、正常および主要不整脈（AF / PAC / PVC）を分類する。

## Core Capabilities

- ECL からの拍単位解析（拍種別 N / PAC / PVC、リズム SR / AF/AFL、Unknown・short RUN フラグ）
- 解析オペレータ向けの CLI（第一面）
- 同一コアからの HTTP API 化（第二面）と、同一バイナリから配信する日本語の簡易コンソール UI（`/ui/`）
- ライセンス連携: 起動時の有効性確認と、解析 1 ジョブあたり 1 回・推論直前の利用記録。いずれも fail-closed
- モデルのビルド時埋め込みによる配布（生 ONNX を配布物に含めない）
- 配布パッケージ（Windows インストーラ / Docker イメージ / Linux deb・rpm）
- 推論 EP の実行時選択（`auto` = CUDA → CPU）と GPU 高速化（まとめ推論・CUDA チューニング・HTTP モデル常駐）、CPU FP32 基準との精度・速度比較レポート（`compare-accel`）

## Target Use Cases

- ホルター解析センターでの一次スクリーニング／分類補助
- バッチ入力ファイルに対するラベル付与と結果出力（`beat_results.csv` / JSON）
- 連携システムからの HTTP 解析、および導入時のブラウザでの疎通確認

## Value Proposition

仕様（cc-sdd）とコード境界を明示し、CLI → API への段階導入を可能にする。  
推論モデル差し替え時も入出力契約を維持する。

---
_Focus on patterns and purpose, not exhaustive feature lists_
