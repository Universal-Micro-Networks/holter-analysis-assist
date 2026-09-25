#!/usr/bin/env bash
# FpmBuilder: build Linux x86_64 .deb and .rpm from the common staging tree.
#
# Usage:
#   fpm.sh [--staging <dir>] [--output <dir>] [--version <ver>] [--prepare-only]
#
# Default staging: packaging/out/staging/linux
# Default output:  packaging/out/linux
# Default version: 0.1.0
#
# Install layout (after package install):
#   /usr/bin/holter-http-api
#   /usr/share/holter-http-api/NOTICE
#   /usr/share/holter-http-api/http.ini.example
#
# Architectures: deb=amd64, rpm=x86_64
# Both -t deb and -t rpm are required from the same staging input.
#
# --prepare-only: assemble the fpm input root under <output>/root and exit
#                 (no fpm invocation). Used by contract tests and hosts without fpm.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STAGING="${ROOT}/packaging/out/staging/linux"
OUTPUT="${ROOT}/packaging/out/linux"
VERSION="0.1.0"
PREPARE_ONLY=0
PACKAGE_NAME="holter-http-api"

usage() {
  echo "Usage: $0 [--staging <dir>] [--output <dir>] [--version <ver>] [--prepare-only]" >&2
  exit 2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --staging)
      STAGING="${2:-}"
      shift 2
      ;;
    --output)
      OUTPUT="${2:-}"
      shift 2
      ;;
    --version)
      VERSION="${2:-}"
      shift 2
      ;;
    --prepare-only)
      PREPARE_ONLY=1
      shift
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

if [[ ! -d "${STAGING}" ]]; then
  echo "error: staging root is not a directory: ${STAGING}" >&2
  exit 1
fi

BINARY_SRC="${STAGING}/bin/holter-http-api"
NOTICE_SRC="${STAGING}/NOTICE"
INI_SRC="${STAGING}/http.ini.example"

if [[ ! -f "${BINARY_SRC}" ]]; then
  echo "error: missing staging binary: ${BINARY_SRC}" >&2
  exit 1
fi
if [[ ! -f "${NOTICE_SRC}" ]]; then
  echo "error: missing staging NOTICE: ${NOTICE_SRC}" >&2
  exit 1
fi
if [[ ! -f "${INI_SRC}" ]]; then
  echo "error: missing staging sample ini: ${INI_SRC}" >&2
  exit 1
fi

PKG_ROOT="${OUTPUT}/root"
BIN_DST="${PKG_ROOT}/usr/bin/holter-http-api"
SHARE_DIR="${PKG_ROOT}/usr/share/holter-http-api"
NOTICE_DST="${SHARE_DIR}/NOTICE"
INI_DST="${SHARE_DIR}/http.ini.example"

rm -rf "${PKG_ROOT}"
mkdir -p "$(dirname "${BIN_DST}")" "${SHARE_DIR}"

cp "${BINARY_SRC}" "${BIN_DST}"
chmod 755 "${BIN_DST}"
cp "${NOTICE_SRC}" "${NOTICE_DST}"
cp "${INI_SRC}" "${INI_DST}"

# Optional short docs from staging (pointer only; not required by design paths).
if [[ -d "${STAGING}/docs" ]]; then
  mkdir -p "${SHARE_DIR}/docs"
  cp -R "${STAGING}/docs/." "${SHARE_DIR}/docs/"
fi

echo "prepared install root at ${PKG_ROOT}"
echo "  /usr/bin/holter-http-api"
echo "  /usr/share/holter-http-api/NOTICE"
echo "  /usr/share/holter-http-api/http.ini.example"

if [[ "${PREPARE_ONLY}" -eq 1 ]]; then
  exit 0
fi

if ! command -v fpm >/dev/null 2>&1; then
  echo "error: fpm not found; install fpm (gem install fpm) or use --prepare-only" >&2
  exit 1
fi

mkdir -p "${OUTPUT}"

# Same staging-derived root → both package formats (Req 3.1, 3.2).
# Package name: holter-http-api
# Architectures: deb=amd64, rpm=x86_64
echo "building ${PACKAGE_NAME} ${VERSION} .deb (amd64) and .rpm (x86_64)"

fpm -s dir -t deb \
  -n "${PACKAGE_NAME}" \
  -v "${VERSION}" \
  -a amd64 \
  --prefix / \
  -C "${PKG_ROOT}" \
  -p "${OUTPUT}/${PACKAGE_NAME}_${VERSION}_amd64.deb" \
  usr

fpm -s dir -t rpm \
  -n "${PACKAGE_NAME}" \
  -v "${VERSION}" \
  -a x86_64 \
  --prefix / \
  -C "${PKG_ROOT}" \
  -p "${OUTPUT}/${PACKAGE_NAME}-${VERSION}-1.x86_64.rpm" \
  usr

echo "wrote:"
ls -la "${OUTPUT}/${PACKAGE_NAME}"*.deb "${OUTPUT}/${PACKAGE_NAME}"*.rpm 2>/dev/null || \
  ls -la "${OUTPUT}"
