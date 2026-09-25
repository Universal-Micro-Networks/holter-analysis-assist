# Staging layout (packaging-distribution)

This directory documents the common staging tree used by Docker / Inno / fpm
builders. Full assemble + verify scripts land in later tasks; PackagingIniSample
and NoticeBundle inputs are defined here.

## Required bundled files (per OS staging root)

| Path (under staging root) | Source | Notes |
|---------------------------|--------|--------|
| `NOTICE` | `packaging/NOTICE` (NoticeBundle canonical) | ORT / third-party attribution; must be present in every artifact |
| `http.ini.example` (or `config/http.ini.example`) | PackagingIniSample merge of upstream examples | Must include `[http]` and `[license]` |

## Sample ini key ownership (do not redefine)

- `[license]` keys: copy/reference only from `config/license.ini.example` (license-client).
  Required example: `server_url=...`
- `[http]` keys: copy/reference only from `config/http.ini.example` (http-api).
  Required example: `bind=0.0.0.0:8080`
- Packaging must not introduce `packaging.ini.example` as a key canonical.
- Merge into one runtime sample via `packaging/scripts/assemble-ini-sample.sh`
  → writes `packaging/out/staging/sample/http.ini.example`.

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
