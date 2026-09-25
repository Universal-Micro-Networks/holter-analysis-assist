#!/usr/bin/env bash
# InnoBuilder: compile packaging/windows/holter-http-api.iss with ISCC.
#
# Usage:
#   build-inno.sh [--staging <dir>] [--version <ver>] [--output <dir>] [--iscc <path>]
#
# Default staging: packaging/out/staging/windows
# Default version: 0.1.0
# Default output:  packaging/out/windows
#
# Requires Inno Setup 6.x (ISCC) on PATH, or pass --iscc.
# Compile failure / missing ISCC → non-zero exit (never silent success).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STAGING="${ROOT}/packaging/out/staging/windows"
VERSION="0.1.0"
OUTPUT="${ROOT}/packaging/out/windows"
ISS="${ROOT}/packaging/windows/holter-http-api.iss"
ISCC_BIN="${ISCC:-}"

usage() {
  echo "Usage: $0 [--staging <dir>] [--version <ver>] [--output <dir>] [--iscc <path>]" >&2
  exit 2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --staging)
      STAGING="${2:-}"
      shift 2
      ;;
    --version)
      VERSION="${2:-}"
      shift 2
      ;;
    --output)
      OUTPUT="${2:-}"
      shift 2
      ;;
    --iscc)
      ISCC_BIN="${2:-}"
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

if [[ ! -f "${ISS}" ]]; then
  echo "error: missing Inno script: ${ISS}" >&2
  exit 1
fi

if [[ ! -d "${STAGING}" ]]; then
  echo "error: staging root is not a directory: ${STAGING}" >&2
  exit 1
fi

if [[ ! -f "${STAGING}/bin/holter-http-api.exe" ]]; then
  echo "error: missing staging binary: ${STAGING}/bin/holter-http-api.exe" >&2
  exit 1
fi

if [[ ! -f "${STAGING}/NOTICE" ]]; then
  echo "error: missing staging NOTICE: ${STAGING}/NOTICE" >&2
  exit 1
fi

if [[ ! -f "${STAGING}/http.ini.example" ]]; then
  echo "error: missing staging sample ini: ${STAGING}/http.ini.example" >&2
  exit 1
fi

resolve_iscc() {
  if [[ -n "${ISCC_BIN}" ]]; then
    if [[ -x "${ISCC_BIN}" ]] || command -v "${ISCC_BIN}" >/dev/null 2>&1; then
      echo "${ISCC_BIN}"
      return 0
    fi
    echo "error: --iscc path not executable: ${ISCC_BIN}" >&2
    return 1
  fi
  if command -v iscc >/dev/null 2>&1; then
    command -v iscc
    return 0
  fi
  if command -v ISCC.exe >/dev/null 2>&1; then
    command -v ISCC.exe
    return 0
  fi
  # Common Chocolatey / default install locations on Windows runners.
  for candidate in \
    "/c/Program Files (x86)/Inno Setup 6/ISCC.exe" \
    "/c/Program Files/Inno Setup 6/ISCC.exe" \
    "C:/Program Files (x86)/Inno Setup 6/ISCC.exe" \
    "C:/Program Files/Inno Setup 6/ISCC.exe"; do
    if [[ -x "${candidate}" ]] || [[ -f "${candidate}" ]]; then
      echo "${candidate}"
      return 0
    fi
  done
  echo "error: Inno Setup Compiler (ISCC) not found; install Inno Setup 6.x or pass --iscc" >&2
  return 1
}

ISCC_PATH="$(resolve_iscc)" || exit 1

mkdir -p "${OUTPUT}"

echo "compiling ${ISS}"
echo "  staging=${STAGING}"
echo "  version=${VERSION}"
echo "  output=${OUTPUT}"
echo "  iscc=${ISCC_PATH}"

# ISCC returns non-zero on compile failure; set -e propagates it (Req 2.4).
# Do not swallow the exit status.
"${ISCC_PATH}" \
  "/DMyAppVersion=${VERSION}" \
  "/DStagingDir=${STAGING}" \
  "/DOutputDir=${OUTPUT}" \
  "${ISS}"
