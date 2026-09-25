#!/usr/bin/env bash
# PackagingIniSample: merge upstream [license] + [http] examples into a staging
# runtime sample. Does NOT redefine keys — copies section bodies from:
#   config/license.ini.example  (license-client canonical)
#   config/http.ini.example     (http-api canonical)
#
# Output (merged sample, not a packaging-owned key canonical):
#   packaging/out/staging/sample/http.ini.example
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
LICENSE_SRC="${ROOT}/config/license.ini.example"
HTTP_SRC="${ROOT}/config/http.ini.example"
OUT_DIR="${ROOT}/packaging/out/staging/sample"
OUT_FILE="${OUT_DIR}/http.ini.example"

if [[ ! -f "${LICENSE_SRC}" ]]; then
  echo "error: missing upstream license sample: ${LICENSE_SRC}" >&2
  exit 1
fi
if [[ ! -f "${HTTP_SRC}" ]]; then
  echo "error: missing upstream http sample: ${HTTP_SRC}" >&2
  exit 1
fi

mkdir -p "${OUT_DIR}"

{
  cat <<'HDR'
# Merged runtime sample for packaging staging (PackagingIniSample).
# Key names and meanings are NOT owned here — copied from upstream:
#   license section ← config/license.ini.example (license-client)
#   http section    ← config/http.ini.example (http-api)
# Edit server_url / bind (and optional keys) per environment after install.
# Prefer owner-only permissions when api_key is set (e.g. chmod 600).

HDR
  # Emit [license] then [http] from upstream files (strip leading comments-only
  # preamble before each section header; keep in-section comments and keys).
  awk '
    BEGIN { emit=0 }
    /^\[license\]/ { emit=1 }
    emit { print }
  ' "${LICENSE_SRC}"
  echo
  awk '
    BEGIN { emit=0 }
    /^\[http\]/ { emit=1 }
    emit { print }
  ' "${HTTP_SRC}"
} >"${OUT_FILE}"

echo "Wrote ${OUT_FILE}"
