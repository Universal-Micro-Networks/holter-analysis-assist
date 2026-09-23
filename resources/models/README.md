# Model resources

推論用のモデル重み・関連バイナリを置くディレクトリです。  
CLI / 将来の API が実行時にここから読み込みます。

## 配置ファイル

| ファイル | 用途 |
|---|---|
| `phase2_internal_finetuned_eventstrong_noise_20s10s_rev1.weights.h5` | Phase2 fine-tuned 不整脈分類重み（event-strong / noise, 20s/10s, rev1） |

## 注意

- `*.h5` など大きな重みファイルは Git 管理外（`.gitignore`）です
- 配布・デプロイ時は別途パッケージング（CI artifact / オブジェクトストレージ等）で同梱してください
