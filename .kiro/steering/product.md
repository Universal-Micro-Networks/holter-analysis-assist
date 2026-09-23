# Product Overview

検査会社のホルター心電図解析業務を支援する、不整脈リズム AI アプリケーション。  
心電図を入力し、正常および主要不整脈（AF / PAC / PVC）を分類する。

## Core Capabilities

- 心電図入力からのリズム分類（NORMAL / AF / PAC / PVC）
- 解析オペレータ向けの CLI（第一面）
- 同一コアからの HTTP API 化（第二面）

## Target Use Cases

- ホルター解析センターでの一次スクリーニング／分類補助
- バッチ入力ファイルに対するラベル付与と結果出力（text / JSON）

## Value Proposition

仕様（cc-sdd）とコード境界を明示し、CLI → API への段階導入を可能にする。  
推論モデル差し替え時も入出力契約を維持する。

---
_Focus on patterns and purpose, not exhaustive feature lists_
