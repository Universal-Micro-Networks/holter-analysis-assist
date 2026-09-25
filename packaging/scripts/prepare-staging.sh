#!/usr/bin/env bash
# StagingLayout: assemble common packaging staging from embedded HTTP binary,
# PackagingIniSample merge, and NoticeBundle.
#
# Usage:
#   prepare-staging.sh --os linux|windows --binary <path> [--out <dir>]
#
# Default output: packaging/out/staging/<os>/
# Layout:
#   bin/holter-http-api[.exe]
#   http.ini.example   (upstream [http]+[license] merge)
#   NOTICE
#   docs/              (optional pointer; created empty for packagers)
#
# Premise: input binary is the embedded holter-http-api from upstream artifact
# release-embedded-http-api. Raw model files must not be staged.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
OS=""
BINARY=""
OUT=""

usage() {
  echo "Usage: $0 --os linux|windows --binary <path> [--out <staging-dir>]" >&2
  exit 2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --os)
      OS="${2:-}"
      shift 2
      ;;
    --binary)
      BINARY="${2:-}"
      shift 2
      ;;
    --out)
      OUT="${2:-}"
      shift 2
      ;;
    -h|--help)
      usage
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      usage
      ;;
  esac
done

if [[ -z "${OS}" || -z "${BINARY}" ]]; then
  echo "error: --os and --binary are required" >&2
  usage
fi

case "${OS}" in
  linux|windows) ;;
  *)
    echo "error: --os must be linux or windows (got: ${OS})" >&2
    exit 1
    ;;
esac

if [[ ! -f "${BINARY}" ]]; then
  echo "error: binary not found: ${BINARY}" >&2
  exit 1
fi

NOTICE_SRC="${ROOT}/packaging/NOTICE"
if [[ ! -f "${NOTICE_SRC}" ]]; then
  echo "error: missing NoticeBundle: ${NOTICE_SRC}" >&2
  exit 1
fi

if [[ -z "${OUT}" ]]; then
  OUT="${ROOT}/packaging/out/staging/${OS}"
fi

# Assemble merged runtime sample (PackagingIniSample).
bash "${ROOT}/packaging/scripts/assemble-ini-sample.sh"
SAMPLE_SRC="${ROOT}/packaging/out/staging/sample/http.ini.example"
if [[ ! -f "${SAMPLE_SRC}" ]]; then
  echo "error: sample ini missing after assemble: ${SAMPLE_SRC}" >&2
  exit 1
fi

BIN_NAME="holter-http-api"
if [[ "${OS}" == "windows" ]]; then
  BIN_NAME="holter-http-api.exe"
fi

rm -rf "${OUT}"
mkdir -p "${OUT}/bin" "${OUT}/docs"
cp "${BINARY}" "${OUT}/bin/${BIN_NAME}"
cp "${NOTICE_SRC}" "${OUT}/NOTICE"
cp "${SAMPLE_SRC}" "${OUT}/http.ini.example"

# Document embedded-binary premise for packagers consuming this tree.
cat >"${OUT}/docs/EMBEDDED_BINARY.md" <<'EOF'
# Embedded binary premise

This staging tree is built from the upstream embedded HTTP binary
(`holter-http-api` from artifact `release-embedded-http-api`).

Raw model weight files (for example `.onnx`) must not be added here.
Packagers (Docker / Inno / fpm) must consume this layout as-is after
`verify-artifact.sh` succeeds.
EOF

echo "Staging assembled at ${OUT}"
