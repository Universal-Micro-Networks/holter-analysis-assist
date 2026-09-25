# Staging layout (packaging-distribution)

Common staging tree shared by Docker / Inno / fpm builders after
`packaging/scripts/prepare-staging.sh` and before
`packaging/scripts/verify-artifact.sh`.

## Embedded binary premise

Distribution artifacts are built from the **upstream embedded** HTTP binary
(`holter-http-api` from CI artifact `release-embedded-http-api`). Raw model
weight files (for example `.onnx`, `.ort`, `*.weights.h5`) must **not** be
placed in staging or any packaging input. Packagers consume the verified
staging tree only.

## Assemble and verify

```bash
# Assemble (copies binary + NOTICE + merged sample ini)
./packaging/scripts/prepare-staging.sh --os linux --binary /path/to/holter-http-api
./packaging/scripts/prepare-staging.sh --os windows --binary /path/to/holter-http-api.exe

# Verify (fail closed on missing NOTICE/ini/binary or forbidden extensions)
./packaging/scripts/verify-artifact.sh packaging/out/staging/linux
```

Default output root: `packaging/out/staging/<os>/`.

## Required bundled files (per OS staging root)

| Path (under staging root) | Source | Notes |
|---------------------------|--------|--------|
| `bin/holter-http-api` (Linux) or `bin/holter-http-api.exe` (Windows) | Upstream embedded binary from `release-embedded-http-api` | Not a raw model file |
| `NOTICE` | `packaging/NOTICE` (NoticeBundle canonical) | ORT / third-party attribution; must be present in every artifact |
| `http.ini.example` | PackagingIniSample merge of upstream examples | Must include `[http]` and `[license]` |
| `docs/` | Short pointers (e.g. embedded premise) | Optional for runtime; used by packagers |

## Sample ini key ownership (do not redefine)

- `[license]` keys: copy/reference only from `config/license.ini.example` (license-client).
  Required example: `server_url=...`
- `[http]` keys: copy/reference only from `config/http.ini.example` (http-api).
  Required example: `bind=0.0.0.0:8080`
- Packaging must not introduce `packaging.ini.example` as a key canonical.
- Merge into one runtime sample via `packaging/scripts/assemble-ini-sample.sh`
  → writes `packaging/out/staging/sample/http.ini.example`, then copied into
  each OS staging root by `prepare-staging.sh`.

## Operator placement after install

- Copy the sample to the runtime config path expected by `holter-http-api`
  (`--config` / `HOLTER_HTTP_INI`; default `config/http.ini`).
- Set at least `server_url` (license) and `bind` (HTTP listen).
- Key meanings remain as documented in the upstream example comments; this
  packaging layer does not redefine them.
- NOTICE location in artifacts: alongside the binary / under the package share
  directory (exact paths in `docs/packaging/*`, later task).

## Out of scope for this packaging feature

- License server implementation
- HTTP API resource / error contract redesign
- Model embedding logic (consumes embedded binary only)
- Cloud IaC / advanced auto-update channels
