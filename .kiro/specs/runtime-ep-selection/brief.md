# Runtime EP selection (CUDA / CPU)

## Context
開発・解析マシンは macOS（CPU）と Windows（NVIDIA GPU）が混在する。`--provider auto` で **CUDA → CPU** を試し、明示指定 `cuda` / `cpu` も維持する。

## Note
CoreML EP は精度劣化（拍検出崩壊）のため採用しない / コードから除去済み。

## Scope
- `ExecutionProviderKind`: `auto` / `cuda` / `cpu`
- Cargo feature `cuda`（default on）+ `lax-feature-matching`
- CLI / example の `--provider`

## Status
実装済み（コミット `635ff56`、`920bc0a`）。requirements / design / tasks は作らずに直接実装したため、本 brief のみを残し、`.kiro/steering/roadmap.md` の「Implemented Without Spec Flow」で実装済みとして管理する。

文字列からの変換（`FromStr`）は `gpu` / `nvidia` を `cuda` の別名として受け付ける。後続の http-api で追加された ini の `[http] provider` と HTTP の multipart `provider` はこの変換を使うため別名が有効。CLI の `--provider` は `auto` / `cpu` / `cuda` のみ。
