#!/usr/bin/env bash
# ArtifactVerify: gate staging (or expanded artifact) for required bundles and
# forbidden raw model files.
#
# Usage:
#   verify-artifact.sh <staging-root>
#
# Required:
#   NOTICE
#   http.ini.example (sample ini with upstream [http] / [license])
#   bin/holter-http-api  OR  bin/holter-http-api.exe
#
# Forbidden (fail closed):
#   *.onnx, *.ort, *.weights.h5  (and paths containing those suffixes)
#
# Exit 0 on pass; non-zero on failure (message on stderr).
set -euo pipefail

if [[ $# -ne 1 ]]; then
  echo "Usage: $0 <staging-root>" >&2
  exit 2
fi

STAGING="$1"
if [[ ! -d "${STAGING}" ]]; then
  echo "error: staging root is not a directory: ${STAGING}" >&2
  exit 1
fi

fail() {
  echo "error: $*" >&2
  exit 1
}

# --- Required files ---
if [[ ! -f "${STAGING}/NOTICE" ]]; then
  fail "NOTICE missing under ${STAGING}"
fi
if [[ ! -s "${STAGING}/NOTICE" ]]; then
  fail "NOTICE is empty under ${STAGING}"
fi

if [[ ! -f "${STAGING}/http.ini.example" ]]; then
  fail "sample ini (http.ini.example) missing under ${STAGING}"
fi
if [[ ! -s "${STAGING}/http.ini.example" ]]; then
  fail "sample ini (http.ini.example) is empty under ${STAGING}"
fi

BIN_LINUX="${STAGING}/bin/holter-http-api"
BIN_WIN="${STAGING}/bin/holter-http-api.exe"
if [[ ! -f "${BIN_LINUX}" && ! -f "${BIN_WIN}" ]]; then
  fail "binary missing: expected bin/holter-http-api or bin/holter-http-api.exe under ${STAGING}"
fi

# --- Forbidden raw model extensions ---
# List is explicit per design ArtifactVerify (*.onnx, *.ort, *.weights.h5).
FORBIDDEN_FOUND=0
while IFS= read -r -d '' path; do
  echo "error: forbidden raw model file detected: ${path}" >&2
  FORBIDDEN_FOUND=1
done < <(
  find "${STAGING}" \( \
    -name '*.onnx' -o \
    -name '*.ort' -o \
    -name '*.weights.h5' \
  \) -print0 2>/dev/null || true
)

if [[ "${FORBIDDEN_FOUND}" -ne 0 ]]; then
  fail "staging contains forbidden model weight files (e.g. .onnx); refuse packaging"
fi

echo "verify-artifact: OK (${STAGING})"
