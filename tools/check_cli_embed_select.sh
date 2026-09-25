#!/usr/bin/env bash
# CliModelSelect 結合確認 (model-embedding task 5.2)
# Requirements 3.1, 3.3, 4.1, 4.2 / Design: CliModelSelect, Testing Strategy E2E/CLI
#
# After license-client CliStartupIntegration, the CLI requires a reachable license
# server before any subcommand. This script starts a tiny local mock and passes
# --license-config (same pattern as tests/cli_model_select.rs).
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
LICENSE_TMP="$(mktemp -d)"
LICENSE_INI="$LICENSE_TMP/license.ini"
MOCK_PORT_FILE="$LICENSE_TMP/port"
MOCK_PID=""

cleanup() {
  rm -f "$TMP_ERR"
  if [[ -n "${MOCK_PID}" ]] && kill -0 "$MOCK_PID" 2>/dev/null; then
    kill "$MOCK_PID" 2>/dev/null || true
    wait "$MOCK_PID" 2>/dev/null || true
  fi
  rm -rf "$LICENSE_TMP"
}
trap cleanup EXIT

start_mock_license_server() {
  # Tiny allow-all mock: POST any path → {"allowed":true} (matches cli_model_select / cli_license_startup).
  # Write script to disk first — `python <<'PY' &` can race and never consume the heredoc.
  # Bypass HTTPServer.server_bind's socket.getfqdn() — reverse DNS on 127.0.0.1 can hang
  # indefinitely in some agent/CI environments (Python 3.x BaseServer).
  local mock_py="$LICENSE_TMP/mock_license_server.py"
  cat >"$mock_py" <<'PY'
import json
import socket
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port_file = sys.argv[1]
body = json.dumps({"allowed": True}).encode("utf-8")


class NoDnsHTTPServer(HTTPServer):
    """HTTPServer without reverse-DNS getfqdn in server_bind (can hang on 127.0.0.1)."""

    def server_bind(self):
        socket.socket.bind(self.socket, self.server_address)
        self.server_address = self.socket.getsockname()
        host, port = self.server_address[:2]
        self.server_name = host if isinstance(host, str) else "127.0.0.1"
        self.server_port = port


class Handler(BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("Content-Length", "0") or "0")
        if length:
            self.rfile.read(length)
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        pass


httpd = NoDnsHTTPServer(("127.0.0.1", 0), Handler)
_host, port = httpd.server_address[:2]
with open(port_file, "w", encoding="utf-8") as f:
    f.write(str(port))
    f.flush()
httpd.serve_forever()
PY
  local mock_log="$LICENSE_TMP/mock.log"
  # -u: unbuffered so ready/errors show up if startup hangs; log to a file.
  python3 -u "$mock_py" "$MOCK_PORT_FILE" >"$mock_log" 2>&1 &
  MOCK_PID=$!

  # Wait until the server writes its ephemeral port.
  for _ in $(seq 1 200); do
    if [[ -s "$MOCK_PORT_FILE" ]]; then
      break
    fi
    if ! kill -0 "$MOCK_PID" 2>/dev/null; then
      fail "mock license server exited before publishing port; log=$(cat "$mock_log" 2>/dev/null || true)"
    fi
    sleep 0.05
  done
  [[ -s "$MOCK_PORT_FILE" ]] || fail "mock license server did not publish port; pid=$MOCK_PID log=$(cat "$mock_log" 2>/dev/null || true) dir=$(ls -la "$LICENSE_TMP" 2>/dev/null || true)"
  local port
  port="$(cat "$MOCK_PORT_FILE")"
  printf '[license]\nserver_url=http://127.0.0.1:%s\ntimeout_secs=5\n' "$port" >"$LICENSE_INI"
  echo "Mock license server on http://127.0.0.1:${port} (pid=$MOCK_PID)"
}

run_cli() {
  "$BIN" --license-config "$LICENSE_INI" "$@"
}

if [[ ! -f "$MODEL_PATH" ]] || [[ ! -s "$MODEL_PATH" ]]; then
  fail "readable non-empty model required for embed smoke: $MODEL_PATH
  Set HOLTER_EMBEDDED_MODEL_PATH or place phase2_rev1.onnx under resources/models/"
fi

command -v python3 >/dev/null || fail "python3 required to run local mock license HTTP server"

export HOLTER_EMBEDDED_MODEL_PATH="$MODEL_PATH"
echo "Building embedded CLI (CPU, no-default-features) with HOLTER_EMBEDDED_MODEL_PATH=$MODEL_PATH"
# Match release-embedded-cli: CPU artifact without cuda default.
if ! cargo build -q --no-default-features --features embedded-model 2>"$TMP_ERR"; then
  fail "embedded-model build failed: $(cat "$TMP_ERR")"
fi
[[ -x "$BIN" ]] || fail "expected binary at $BIN"

start_mock_license_server

# --- Case 1: embedded default (no --model) infer-window smoke ---
if run_cli infer-window --provider cpu >"$TMP_ERR" 2>&1; then
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
run_cli infer-window --model "$MISSING_MODEL" --provider cpu >"$TMP_ERR" 2>&1
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
run_cli analyze-ecl /tmp/1234567890_20240101_0000_2359.ecl \
  --model "$MISSING_MODEL" --provider cpu \
  --output /tmp/holter-cli-embed-select-out.csv >"$TMP_ERR" 2>&1
rc=$?
set -e
[[ "$rc" -ne 0 ]] || fail "analyze-ecl missing --model must exit non-zero"
grep -q 'ONNX model not found' "$TMP_ERR" \
  || fail "analyze-ecl stderr must contain 'ONNX model not found'; got: $(cat "$TMP_ERR")"
pass "analyze-ecl --model missing path → non-zero + clear message"

echo "All CliModelSelect CLI embed/path checks passed."
