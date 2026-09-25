#!/usr/bin/env bash
# DockerBuilder: wire .dockerignore into the Linux staging build context, then
# build the CPU-default image for linux/amd64.
#
# Usage:
#   build-docker.sh [--staging <dir>] [--tag <name>] [--prepare-only]
#
# Default staging: packaging/out/staging/linux
# Default tag:     holter-http-api:dev-cpu
#
# --prepare-only: copy packaging/docker/.dockerignore into the staging context
#                 root and exit (no docker build). Used by contract tests and
#                 for hosts without Docker Engine.
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
STAGING="${ROOT}/packaging/out/staging/linux"
TAG="holter-http-api:dev-cpu"
PREPARE_ONLY=0
DOCKERFILE="${ROOT}/packaging/docker/Dockerfile"
DOCKERIGNORE_SRC="${ROOT}/packaging/docker/.dockerignore"

usage() {
  echo "Usage: $0 [--staging <dir>] [--tag <name>] [--prepare-only]" >&2
  exit 2
}

while [[ $# -gt 0 ]]; do
  case "$1" in
    --staging)
      STAGING="${2:-}"
      shift 2
      ;;
    --tag)
      TAG="${2:-}"
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

if [[ ! -f "${DOCKERIGNORE_SRC}" ]]; then
  echo "error: missing .dockerignore: ${DOCKERIGNORE_SRC}" >&2
  exit 1
fi

if [[ ! -f "${DOCKERFILE}" ]]; then
  echo "error: missing Dockerfile: ${DOCKERFILE}" >&2
  exit 1
fi

# Docker reads .dockerignore from the build *context* root (not -f path).
# Wire the canonical ignore file into staging before build.
cp "${DOCKERIGNORE_SRC}" "${STAGING}/.dockerignore"
echo "wired ${DOCKERIGNORE_SRC} -> ${STAGING}/.dockerignore"

if [[ "${PREPARE_ONLY}" -eq 1 ]]; then
  exit 0
fi

# Prefer explicit linux/amd64 (Req 1.4 / design: Linux x86_64).
exec docker build \
  --platform linux/amd64 \
  -f "${DOCKERFILE}" \
  -t "${TAG}" \
  "${STAGING}"
