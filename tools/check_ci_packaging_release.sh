#!/usr/bin/env bash
# ReleasePublish 契約検証 (packaging-distribution task 3.1)
# Requirements 5.1, 5.2, 5.3, 5.4, 8.1, 8.2, 8.3, 8.4, 4.3, 7.3
# Design: ReleasePublish
#
# Checks that .github/workflows/ci.yml defines packaging jobs that:
# - Consume upstream artifact release-embedded-http-api (embedded holter-http-api)
# - Document: required packaging gate applies only when upstream succeeds;
#   soft-skip of release-embedded-http-api ⇒ packaging also skips (not fail-open)
# - Fail when packaging runs but input is missing (no silent success)
# - Install Inno Setup on Windows / fpm (+ rpm tooling) on Linux
# - Run ArtifactVerify before Docker / Inno / deb / rpm builds (YAML step order)
# - Upload CPU-default artifacts with OS / arch / cpu identifiable names
# - Keep CUDA optional and separate from the required CPU gate
# - Do NOT redefine model-embedding's release-embedded-cli
# - Document job / artifact names (input + packaging OWN)
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

extract_job() {
  local job_id="$1"
  awk -v id="$job_id" '
    $0 ~ "^  " id ":" { grab=1 }
    grab && /^  [a-zA-Z0-9_-]+:/ && $0 !~ "^  " id ":" { exit }
    grab { print }
  ' "$WF"
}

[[ -f "$WF" ]] || fail "workflow missing: $WF"

# --- Packaging OWN jobs (design Batch / Job Contract) ---
PKG_WIN="$(extract_job package-windows)"
PKG_LINUX="$(extract_job package-linux)"

[[ -n "$PKG_WIN" ]] || fail "job 'package-windows' not found in $WF"
pass "job id package-windows exists"

[[ -n "$PKG_LINUX" ]] || fail "job 'package-linux' not found in $WF"
pass "job id package-linux exists"

# Job-name documentation (input + packaging OWN) somewhere in the packaging section.
PKG_DOCS="$(
  awk '
    /^  package-windows:/ { grab=1 }
    /^  package-linux:/ { grab=1 }
    grab && /^  [a-zA-Z0-9_-]+:/ && !/^  package-(windows|linux):/ { if (seen_linux) exit }
    grab && /^  package-linux:/ { seen_linux=1 }
    grab { print }
  ' "$WF"
)"
# Also accept comments near either packaging job; scan a wider window once jobs exist.
PKG_SECTION="$(
  awk '
    /^  # .*[Pp]ackag|^  package-windows:|^  package-linux:/ { grab=1 }
    grab && /^  [a-zA-Z0-9_-]+:/ && !/^  package-(windows|linux):/ && !/^  #/ {
      if (after_pkg) exit
    }
    grab && /^  package-(windows|linux):/ { after_pkg=1 }
    grab { print }
  ' "$WF"
)"
DOC_BLOB="${PKG_SECTION}"$'\n'"${PKG_WIN}"$'\n'"${PKG_LINUX}"

echo "$DOC_BLOB" | grep -Fq 'release-embedded-http-api' \
  || fail "must document input artifact/job name release-embedded-http-api"
echo "$DOC_BLOB" | grep -Eq 'package-windows|package-linux|packaging-distribution' \
  || fail "must document packaging OWN job names (package-windows / package-linux)"
pass "job names documented (input release-embedded-http-api + packaging OWN)"

# Soft-skip policy (remediation A): required gate only when upstream succeeds;
# upstream soft-skip ⇒ packaging skips too (not fail-open surprise).
if ! echo "$DOC_BLOB" | grep -Eiq 'soft-skip'; then
  fail "must document soft-skip: upstream soft-skip ⇒ packaging also skips"
fi
if ! echo "$DOC_BLOB" | grep -Eiq \
  'gate.*succeed|succeed.*gate|only when.*succeed|when.*release-embedded-http-api.*succeed|applies only when'; then
  fail "must document that packaging required gate applies only when release-embedded-http-api succeeds"
fi
if ! echo "$DOC_BLOB" | grep -Eiq \
  'also skip|packaging also skip|skips via needs|soft-skip.*packaging|packaging.*soft-skip'; then
  fail "must document that soft-skip of upstream means packaging also skips"
fi
pass "soft-skip / required-gate policy documented (skip with upstream; not fail-open)"

# --- Consume upstream artifact; fail if missing when packaging runs ---
for block_name in package-windows package-linux; do
  BLOCK="$(extract_job "$block_name")"
  echo "$BLOCK" | grep -Eq 'needs:[[:space:]]*(\[)?[[:space:]]*release-embedded-http-api' \
    || fail "$block_name must needs: release-embedded-http-api (skip when upstream soft-skips)"
  echo "$BLOCK" | grep -Eq 'download-artifact|actions/download-artifact' \
    || fail "$block_name must download-artifact release-embedded-http-api"
done
pass "packaging jobs needs: + download release-embedded-http-api"

# Missing input must fail the packaging job when it runs (fail-closed).
MISSING_FAIL=0
for BLOCK in "$PKG_WIN" "$PKG_LINUX"; do
  if echo "$BLOCK" | grep -Eqi 'if-no-files-found:[[:space:]]*error|exit 1|fail.*(missing|not found)|error.*(missing|not found)'; then
    MISSING_FAIL=1
  fi
  # download-artifact@v4 fails by default when the named artifact is absent;
  # require explicit artifact name referencing the upstream family.
  if echo "$BLOCK" | grep -Eq 'name:[[:space:]]*release-embedded-http-api'; then
    MISSING_FAIL=1
  fi
done
[[ "$MISSING_FAIL" -eq 1 ]] \
  || fail "packaging must fail when release-embedded-http-api input is missing"
pass "missing upstream artifact fails packaging when job runs (fail-closed)"

# --- Tool installs ---
echo "$PKG_WIN" | grep -Eiq 'Inno|innosetup|ISCC' \
  || fail "package-windows must install / use Inno Setup (ISCC)"
# Explicit install step (choco / winget / download), not only a path check.
echo "$PKG_WIN" | grep -Eiq 'choco[[:space:]]+install|winget[[:space:]]+install|innosetup' \
  || fail "package-windows must explicitly install Inno Setup on the runner"
pass "package-windows installs Inno Setup"

echo "$PKG_LINUX" | grep -Eiq '\bfpm\b' \
  || fail "package-linux must install / use fpm"
echo "$PKG_LINUX" | grep -Eiq 'gem[[:space:]]+install[[:space:]]+fpm|apt.*fpm|install.*fpm' \
  || fail "package-linux must explicitly install fpm"
# rpm generation needs rpm tooling on Debian runners.
echo "$PKG_LINUX" | grep -Eiq '\brpm\b' \
  || fail "package-linux must install rpm tooling for .rpm generation"
pass "package-linux installs fpm and rpm tooling"

# --- Verify then build (presence + YAML step order) ---
verify_before_build() {
  local block_name="$1"
  local build_pattern="$2"
  local BLOCK
  BLOCK="$(extract_job "$block_name")"
  echo "$BLOCK" | grep -Fq 'verify-artifact' \
    || fail "$block_name must run verify-artifact.sh before packaging builds"
  echo "$BLOCK" | grep -Fq 'prepare-staging' \
    || fail "$block_name must prepare staging from the downloaded binary"

  # Step order: first "- name:" line that mentions verify must precede first build step.
  local verify_line build_line
  verify_line="$(echo "$BLOCK" | grep -nE '^[[:space:]]*-[[:space:]]*name:.*[Vv]erify|verify-artifact' | head -n1 | cut -d: -f1 || true)"
  build_line="$(echo "$BLOCK" | grep -nE "$build_pattern" | head -n1 | cut -d: -f1 || true)"
  [[ -n "$verify_line" ]] || fail "$block_name: could not locate verify step line for order check"
  [[ -n "$build_line" ]] || fail "$block_name: could not locate build step line for order check"
  if (( verify_line >= build_line )); then
    fail "$block_name: verify step (line $verify_line) must appear before build step (line $build_line) in YAML"
  fi
}

verify_before_build package-windows 'build-inno|Build Inno|holter-http-api\.iss'
pass "package-windows: verify-artifact appears before Inno build in YAML"

verify_before_build package-linux 'build-docker|Build Docker|docker build|docker save'
# Also ensure verify precedes fpm/deb/rpm packaging step.
PKG_LINUX_VERIFY_LINE="$(echo "$PKG_LINUX" | grep -nE 'verify-artifact|[Vv]erify' | head -n1 | cut -d: -f1)"
PKG_LINUX_FPM_LINE="$(echo "$PKG_LINUX" | grep -nE 'fpm\.sh|Build deb|Build rpm' | head -n1 | cut -d: -f1)"
[[ -n "$PKG_LINUX_FPM_LINE" ]] || fail "package-linux: could not locate deb/rpm build step for order check"
if (( PKG_LINUX_VERIFY_LINE >= PKG_LINUX_FPM_LINE )); then
  fail "package-linux: verify (line $PKG_LINUX_VERIFY_LINE) must appear before deb/rpm build (line $PKG_LINUX_FPM_LINE)"
fi
pass "package-linux: verify-artifact appears before Docker and deb/rpm builds in YAML"

echo "$PKG_WIN" | grep -Eiq 'build-inno|holter-http-api\.iss|ISCC' \
  || fail "package-windows must build the Inno installer after verify"
pass "package-windows builds Inno installer"

echo "$PKG_LINUX" | grep -Eiq 'build-docker|docker build' \
  || fail "package-linux must build Docker image after verify"
echo "$PKG_LINUX" | grep -Eiq 'fpm\.sh|\bfpm\b' \
  || fail "package-linux must build deb/rpm via fpm after verify"
# Both package formats required by task 3.1 / design.
echo "$PKG_LINUX" | grep -Eiq '\.deb|deb' \
  || fail "package-linux must produce .deb"
echo "$PKG_LINUX" | grep -Eiq '\.rpm|rpm' \
  || fail "package-linux must produce .rpm"
pass "package-linux builds Docker, deb, and rpm after verify"

# --- Upload identifiable CPU artifacts (OS / arch / cpu) ---
echo "$PKG_WIN" | grep -Fq 'upload-artifact' \
  || fail "package-windows must upload-artifact the installer"
echo "$PKG_LINUX" | grep -Fq 'upload-artifact' \
  || fail "package-linux must upload-artifact docker/deb/rpm outputs"

# Artifact names must encode OS, arch, and cpu variant (Req 5.4 / 8.2).
WIN_ART="$(echo "$PKG_WIN" | grep -E 'name:[[:space:]]*' | grep -Eiv 'release-embedded-http-api' || true)"
echo "$WIN_ART" | grep -Eiq 'windows' \
  || fail "package-windows upload name must identify windows OS"
echo "$WIN_ART" | grep -Eiq 'x86_64|amd64' \
  || fail "package-windows upload name must identify x86_64/amd64 arch"
echo "$WIN_ART" | grep -Eiq 'cpu' \
  || fail "package-windows upload name must identify cpu variant"

LINUX_ART="$(echo "$PKG_LINUX" | grep -E 'name:[[:space:]]*' | grep -Eiv 'release-embedded-http-api' || true)"
echo "$LINUX_ART" | grep -Eiq 'linux' \
  || fail "package-linux upload name(s) must identify linux OS"
echo "$LINUX_ART" | grep -Eiq 'x86_64|amd64' \
  || fail "package-linux upload name(s) must identify x86_64/amd64 arch"
echo "$LINUX_ART" | grep -Eiq 'cpu' \
  || fail "package-linux upload name(s) must identify cpu variant"
pass "uploaded artifacts identify OS / arch / cpu"

# --- CUDA optional, separate from required CPU gate ---
# Required packaging jobs must be CPU (names/comments) and must not need a CUDA job.
for BLOCK in "$PKG_WIN" "$PKG_LINUX"; do
  if echo "$BLOCK" | grep -Eiq 'needs:.*cuda|cuda.*needs'; then
    fail "required packaging jobs must not depend on a CUDA packaging job"
  fi
done
# Document that CUDA is optional / separate (Req 5.2, 5.3).
if ! echo "$DOC_BLOB" | grep -Eiq 'cuda.*(optional|separate|任意|分離)|optional.*cuda|CUDA.*(optional|separate)'; then
  fail "must document that CUDA packaging is optional/separate from the CPU gate"
fi
pass "CUDA kept optional/separate from required CPU packaging gate"

# --- Do not redefine release-embedded-cli ---
grep -qE '^  release-embedded-cli:' "$WF" \
  || fail "existing release-embedded-cli job must be retained (Requirement 8.4)"
# Packaging jobs must not re-upload or redefine the CLI artifact family.
for BLOCK in "$PKG_WIN" "$PKG_LINUX"; do
  if echo "$BLOCK" | grep -Eq 'name:[[:space:]]*release-embedded-cli'; then
    fail "packaging must not redefine/upload release-embedded-cli"
  fi
  if echo "$BLOCK" | grep -Fq 'holter-analysis-assist' && echo "$BLOCK" | grep -Fq 'upload-artifact'; then
    # Narrow: uploading the CLI binary path is out of scope.
    if echo "$BLOCK" | grep -E 'path:.*holter-analysis-assist'; then
      fail "packaging must not publish holter-analysis-assist CLI (release-embedded-cli OWN)"
    fi
  fi
done
pass "release-embedded-cli retained and not redefined by packaging"

# Upstream http-api job retained as input producer.
grep -qE '^  release-embedded-http-api:' "$WF" \
  || fail "upstream release-embedded-http-api job must remain (packaging consumes it)"
pass "release-embedded-http-api producer job retained"

grep -qE '^  build-test:' "$WF" \
  || fail "existing build-test job must be retained"
pass "build-test job retained"

echo "All ReleasePublish (packaging CI) contract checks passed."
