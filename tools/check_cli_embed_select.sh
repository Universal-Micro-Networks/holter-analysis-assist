#!/usr/bin/env bash
# CliModelSelect 結合確認 (model-embedding task 5.2)
# Requirements 3.1, 3.3, 4.1, 4.2 / Design: CliModelSelect, Testing Strategy E2E/CLI
#
# 1) embedded-model build: infer-window without --model succeeds (smoke)
# 2) --model missing path → non-zero exit + clear "ONNX model not found" message
#
# Uses project-local toolchain (.cargo-tools). Needs a readable ONNX for (1);
# default: resources/models/phase2_rev1.onnx (override with HOLTER_EMBEDDED_MODEL_PATH).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT"

export CARGO_HOME="${CARGO_HOME:-$ROOT/.cargo-tools}"
export RUSTUP_HOME="${RUSTUP_HOME:-$ROOT/.rustup-tools}"
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$ROOT/target}"
# shellcheck disable=SC1091
source "$CARGO_HOME/env"

fail() {
  echo "FAIL: $*" >&2
  exit 1
}

pass() {
  echo "PASS: $*"
}

MODEL_PATH="${HOLTER_EMBEDDED_MODEL_PATH:-$ROOT/resources/models/phase2_rev1.onnx}"
BIN="$CARGO_TARGET_DIR/debug/holter-analysis-assist"
MISSING_MODEL="/tmp/holter-cli-embed-select-missing-$$.onnx"
TMP_ERR="$(mktemp)"
trap 'rm -f "$TMP_ERR"' EXIT

if [[ ! -f "$MODEL_PATH" ]] || [[ ! -s "$MODEL_PATH" ]]; then
  fail "readable non-empty model required for embed smoke: $MODEL_PATH
  Set HOLTER_EMBEDDED_MODEL_PATH or place phase2_rev1.onnx under resources/models/"
fi

export HOLTER_EMBEDDED_MODEL_PATH="$MODEL_PATH"
echo "Building embedded CLI (CPU, no-default-features) with HOLTER_EMBEDDED_MODEL_PATH=$MODEL_PATH"
# Match release-embedded-cli: CPU artifact without cuda default.
if ! cargo build -q --no-default-features --features embedded-model 2>"$TMP_ERR"; then
  fail "embedded-model build failed: $(cat "$TMP_ERR")"
fi
[[ -x "$BIN" ]] || fail "expected binary at $BIN"

# --- Case 1: embedded default (no --model) infer-window smoke ---
if "$BIN" infer-window --provider cpu >"$TMP_ERR" 2>&1; then
  if grep -Eqi 'rhythm|beat|score|class' "$TMP_ERR"; then
    pass "embedded infer-window without --model succeeds (smoke output present)"
  else
    # Still success exit; dump for debugging if format changes.
    pass "embedded infer-window without --model exits 0"
    cat "$TMP_ERR"
  fi
else
  fail "embedded infer-window without --model should succeed; output=$(cat "$TMP_ERR")"
fi

# --- Case 2: --model missing path → non-zero + clear message ---
set +e
"$BIN" infer-window --model "$MISSING_MODEL" --provider cpu >"$TMP_ERR" 2>&1
rc=$?
set -e
[[ "$rc" -ne 0 ]] || fail "missing --model path must exit non-zero (got 0); output=$(cat "$TMP_ERR")"
grep -q 'ONNX model not found' "$TMP_ERR" \
  || fail "stderr must contain 'ONNX model not found'; got: $(cat "$TMP_ERR")"
grep -Fq "$MISSING_MODEL" "$TMP_ERR" \
  || fail "stderr must mention missing path; got: $(cat "$TMP_ERR")"
pass "infer-window --model missing path → exit $rc + clear message"

# Same Path-priority failure on analyze-ecl (valid filename → model fail-fast before ECL I/O).
set +e
"$BIN" analyze-ecl /tmp/1234567890_20240101_0000_2359.ecl \
  --model "$MISSING_MODEL" --provider cpu \
  --output /tmp/holter-cli-embed-select-out.csv >"$TMP_ERR" 2>&1
rc=$?
set -e
[[ "$rc" -ne 0 ]] || fail "analyze-ecl missing --model must exit non-zero"
grep -q 'ONNX model not found' "$TMP_ERR" \
  || fail "analyze-ecl stderr must contain 'ONNX model not found'; got: $(cat "$TMP_ERR")"
pass "analyze-ecl --model missing path → non-zero + clear message"

echo "All CliModelSelect CLI embed/path checks passed."
