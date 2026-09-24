# Runtime EP selection (CUDA / CPU)

## Context
開発・解析マシンは macOS（CPU）と Windows（NVIDIA GPU）が混在する。`--provider auto` で **CUDA → CPU** を試し、明示指定 `cuda` / `cpu` も維持する。

## Note
CoreML EP は精度劣化（拍検出崩壊）のため採用しない / コードから除去済み。

## Scope
- `ExecutionProviderKind`: `auto` / `cuda` / `cpu`
- Cargo feature `cuda`（default on）+ `lax-feature-matching`
- CLI / example の `--provider`
