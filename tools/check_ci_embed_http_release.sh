#!/usr/bin/env bash
# CiEmbedHttpRelease 契約検証 (http-api task 4.2)
# Requirements 2.2, 11.1, 11.2, 11.3
# Design: CiEmbedHttpRelease
#
# Checks that .github/workflows/ci.yml defines release-embedded-http-api with:
# - Win + Linux x86_64 matrix
# - Production-size inject via HOLTER_EMBEDDED_MODEL_URL (curl download; optional AUTH header)
# - Secondary fixture path via HOLTER_EMBEDDED_MODEL_B64 (dev only; GH secret ~48KB limit)
# - Job if: runs when URL OR B64 is non-empty (soft-skip when neither set)
# - release build: --bin holter-http-api --no-default-features --features embedded-model
# - binary size logging
# - artifact named release-embedded-http-api* uploading holter-http-api only (no .onnx)
# - release-embedded-cli job retained unchanged (separate job; not redefined here)
# - existing build-test job retained
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
WF="$ROOT/.github/workflows/ci.yml"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

pass() {
  echo "PASS: $*"
}

[[ -f "$WF" ]] || fail "workflow missing: $WF"

# Extract the release-embedded-http-api job block (until next top-level job or EOF).
# shellcheck disable=SC2016
JOB_BLOCK="$(
  awk '
    /^  release-embedded-http-api:/ { grab=1 }
    grab && /^  [a-zA-Z0-9_-]+:/ && !/^  release-embedded-http-api:/ { exit }
    grab { print }
  ' "$WF"
)"

[[ -n "$JOB_BLOCK" ]] || fail "job 'release-embedded-http-api' not found in $WF"
pass "job id release-embedded-http-api exists"

echo "$JOB_BLOCK" | grep -q 'ubuntu-latest' \
  || fail "release-embedded-http-api matrix must include ubuntu-latest (Linux x86_64)"
echo "$JOB_BLOCK" | grep -q 'windows-latest' \
  || fail "release-embedded-http-api matrix must include windows-latest (Windows x86_64)"
echo "$JOB_BLOCK" | grep -q 'x86_64-unknown-linux-gnu' \
  || fail "release-embedded-http-api matrix must include x86_64-unknown-linux-gnu"
echo "$JOB_BLOCK" | grep -q 'x86_64-pc-windows-msvc' \
  || fail "release-embedded-http-api matrix must include x86_64-pc-windows-msvc"
pass "Win + Linux x86_64 matrix targets present"

# Primary production path: URL download (not B64-only).
echo "$JOB_BLOCK" | grep -q 'HOLTER_EMBEDDED_MODEL_URL' \
  || fail "must reference secrets.HOLTER_EMBEDDED_MODEL_URL for production-size model inject"
echo "$JOB_BLOCK" | grep -Eq 'curl[[:space:]]+(-[A-Za-z]*f|-fsSL|.*-f)' \
  || fail "URL inject path must download model with curl (e.g. curl -fsSL)"
echo "$JOB_BLOCK" | grep -q 'HOLTER_EMBEDDED_MODEL_AUTH_HEADER' \
  || fail "must support optional secrets.HOLTER_EMBEDDED_MODEL_AUTH_HEADER for Authorization"
pass "URL-based inject path present (curl + optional AUTH header)"

# Secondary fixture path (dev only; document size limit)
echo "$JOB_BLOCK" | grep -q 'HOLTER_EMBEDDED_MODEL_B64' \
  || fail "must retain secondary HOLTER_EMBEDDED_MODEL_B64 fixture path"
if ! echo "$JOB_BLOCK" | grep -Eiq '48KB|48 KiB|~48|secret.*size|size limit|GitHub Actions secret'; then
  fail "must document HOLTER_EMBEDDED_MODEL_B64 size limit (~48KB GH secret max)"
fi
pass "secondary B64 fixture path retained with size-limit documentation"

# Soft-skip: job if runs when URL OR B64 non-empty
if ! echo "$JOB_BLOCK" | grep -F "secrets.HOLTER_EMBEDDED_MODEL_URL" | grep -q "!="; then
  fail "job if: must gate on secrets.HOLTER_EMBEDDED_MODEL_URL != ''"
fi
if ! echo "$JOB_BLOCK" | grep -F "secrets.HOLTER_EMBEDDED_MODEL_B64" | grep -q "!="; then
  fail "job if: must gate on secrets.HOLTER_EMBEDDED_MODEL_B64 != ''"
fi
JOB_IF_LINE="$(echo "$JOB_BLOCK" | grep -E '^[[:space:]]*if:' | head -n1 || true)"
[[ -n "$JOB_IF_LINE" ]] || fail "job must have an if: condition for soft-skip"
echo "$JOB_IF_LINE" | grep -Fq 'HOLTER_EMBEDDED_MODEL_URL' \
  || fail "job if: must include HOLTER_EMBEDDED_MODEL_URL"
echo "$JOB_IF_LINE" | grep -Fq 'HOLTER_EMBEDDED_MODEL_B64' \
  || fail "job if: must include HOLTER_EMBEDDED_MODEL_B64"
echo "$JOB_IF_LINE" | grep -Eq '\|\||or' \
  || fail "job if: must OR URL and B64 (run when either is non-empty)"
pass "job if: runs when URL OR B64 non-empty (soft-skip when neither)"

echo "$JOB_BLOCK" | grep -q 'HOLTER_EMBEDDED_MODEL_PATH' \
  || fail "must set HOLTER_EMBEDDED_MODEL_PATH for build.rs inject"
pass "HOLTER_EMBEDDED_MODEL_PATH used for build inject"

echo "$JOB_BLOCK" | grep -q -- '--bin holter-http-api' \
  || fail "release build must target --bin holter-http-api"
echo "$JOB_BLOCK" | grep -q -- '--no-default-features' \
  || fail "release build must use --no-default-features (CPU artifact on GH runners)"
echo "$JOB_BLOCK" | grep -q -- '--features embedded-model' \
  || fail "release build must enable --features embedded-model"
echo "$JOB_BLOCK" | grep -Eq 'cargo build[[:space:]]+--release|--release' \
  || fail "must run cargo build --release"
pass "release build uses --bin holter-http-api --no-default-features --features embedded-model"

# Size measurement
echo "$JOB_BLOCK" | grep -Eiq 'size|wc -c|stat ' \
  || fail "must log release binary size"
pass "binary size measurement step present"

echo "$JOB_BLOCK" | grep -q 'upload-artifact' \
  || fail "must upload-artifact for release-embedded-http-api"
echo "$JOB_BLOCK" | grep -Eq 'name:[[:space:]]*release-embedded-http-api' \
  || fail "artifact name must be release-embedded-http-api (or release-embedded-http-api-<suffix>)"
pass "artifact name uses release-embedded-http-api"

# Artifact path must be the HTTP binary, not a broad dir that could include .onnx
if echo "$JOB_BLOCK" | grep -E 'path:.*\.onnx|path:.*resources/models|\*\*/\*\.onnx'; then
  fail "artifact must not include .onnx / resources/models paths"
fi
echo "$JOB_BLOCK" | grep -Eq 'holter-http-api(\.exe)?' \
  || fail "artifact path must target holter-http-api binary"
pass "artifact targets holter-http-api binary only (no .onnx paths)"

# Out of scope for this job (Requirement 11.3 / design: packaging OWN elsewhere)
for forbidden in 'Inno' 'fpm' 'docker build' 'license-server' 'packaging-distribution'; do
  if echo "$JOB_BLOCK" | grep -Fqi "$forbidden"; then
    fail "release-embedded-http-api must not include out-of-scope step mentioning: $forbidden"
  fi
done
pass "no packaging / license packaging steps in job"

# Requirement 11.3: do not own / redefine release-embedded-cli
grep -qE '^  release-embedded-cli:' "$WF" \
  || fail "existing release-embedded-cli job must be retained (Requirement 11.3)"
pass "release-embedded-cli job retained (not owned by this job)"

grep -qE '^  build-test:' "$WF" \
  || fail "existing build-test job must be retained"
pass "build-test job retained"

echo "All CiEmbedHttpRelease contract checks passed."
