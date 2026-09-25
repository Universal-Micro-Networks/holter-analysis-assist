#!/usr/bin/env bash
# EmbedBuildGate 検証 (model-embedding task 1.1 / Requirements 5.1, 5.2)
# feature 有効時は HOLTER_EMBEDDED_MODEL_PATH 必須。無効時は環境変数なしでビルド成功。
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export CARGO_HOME="${CARGO_HOME:-$ROOT/.cargo-tools}"
export RUSTUP_HOME="${RUSTUP_HOME:-$ROOT/.rustup-tools}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
# shellcheck disable=SC1091
source "$CARGO_HOME/env"

TMPDIR_GATE="$(mktemp -d)"
trap 'rm -rf "$TMPDIR_GATE"' EXIT

MODEL_OK="$TMPDIR_GATE/ok.onnx"
printf 'fake-model-bytes' >"$MODEL_OK"
EMPTY_FILE="$TMPDIR_GATE/empty.onnx"
: >"$EMPTY_FILE"
NOT_A_FILE="$TMPDIR_GATE/not-a-file-dir"
mkdir -p "$NOT_A_FILE"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

pass() {
  echo "PASS: $*"
}

# --- Case A: default (no embedded-model) succeeds without env ---
unset HOLTER_EMBEDDED_MODEL_PATH || true
if cargo build -q 2>"$TMPDIR_GATE/default.err"; then
  pass "default build succeeds without HOLTER_EMBEDDED_MODEL_PATH"
else
  fail "default build should succeed; stderr=$(cat "$TMPDIR_GATE/default.err")"
fi

# --- Case B: embedded-model without env must FAIL ---
unset HOLTER_EMBEDDED_MODEL_PATH || true
if cargo build -q --features embedded-model 2>"$TMPDIR_GATE/missing.err"; then
  fail "embedded-model build without HOLTER_EMBEDDED_MODEL_PATH must fail"
else
  pass "embedded-model build fails when HOLTER_EMBEDDED_MODEL_PATH is unset"
fi

# --- Case C: empty env must FAIL ---
export HOLTER_EMBEDDED_MODEL_PATH=""
if cargo build -q --features embedded-model 2>"$TMPDIR_GATE/empty_env.err"; then
  fail "embedded-model build with empty HOLTER_EMBEDDED_MODEL_PATH must fail"
else
  pass "embedded-model build fails when HOLTER_EMBEDDED_MODEL_PATH is empty"
fi

# --- Case D: empty file must FAIL ---
export HOLTER_EMBEDDED_MODEL_PATH="$EMPTY_FILE"
if cargo build -q --features embedded-model 2>"$TMPDIR_GATE/empty_file.err"; then
  fail "embedded-model build with empty model file must fail"
else
  pass "embedded-model build fails when model file is empty"
fi

# --- Case E: directory (non-file) must FAIL ---
export HOLTER_EMBEDDED_MODEL_PATH="$NOT_A_FILE"
if cargo build -q --features embedded-model 2>"$TMPDIR_GATE/dir.err"; then
  fail "embedded-model build with directory path must fail"
else
  pass "embedded-model build fails when path is not a file"
fi

# --- Case F: readable model file must SUCCEED ---
export HOLTER_EMBEDDED_MODEL_PATH="$MODEL_OK"
if cargo build -q --features embedded-model 2>"$TMPDIR_GATE/ok.err"; then
  pass "embedded-model build succeeds with readable model file"
else
  fail "embedded-model build should succeed with valid file; stderr=$(cat "$TMPDIR_GATE/ok.err")"
fi

echo "All EmbedBuildGate checks passed."
