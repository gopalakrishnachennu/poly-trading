#!/bin/sh
set -eu

project_root=$(CDPATH= cd -- "$(dirname -- "$0")/../.." && pwd)
terminal_root="$project_root/terminal"

cleanup() {
  if [ "${gateway_owned:-0}" -eq 1 ] && [ -n "${gateway_pid:-}" ]; then
    kill "$gateway_pid" 2>/dev/null || true
    wait "$gateway_pid" 2>/dev/null || true
  fi
}
trap cleanup EXIT HUP INT TERM

cd "$project_root"
gateway_owned=0
gateway_pid=""

# Configuration is deliberately file-bound and read once by the gateway.  The
# local files are ignored by Git so an operator can review/adjust paper
# economics without baking them into the binary.  Do not manufacture values
# here: an absent policy must continue to block paper execution.
if [ -f "$project_root/config/terminal-runtime.json" ]; then
  export POLY_TERMINAL_CONFIG_PATH="$project_root/config/terminal-runtime.json"
fi
if [ -f "$project_root/config/paper-market-policy.json" ]; then
  export POLY_PAPER_POLICY_PATH="$project_root/config/paper-market-policy.json"
fi

# Reuse a healthy local gateway when the operator already has one running.
# This prevents a second process from producing a noisy AddrInUse failure and
# avoids killing a process this wrapper did not start during cleanup.
# Prefer an optimized release gateway. A debug build pegs CPU and stalls under
# a long session, which makes the terminal flap to NO_TRADE; release stays idle.
release_bin="$project_root/target/release/terminal-projection"
debug_bin="$project_root/target/debug/terminal-projection"
if command -v curl >/dev/null 2>&1 && curl -fsS --max-time 1 http://127.0.0.1:8088/healthz >/dev/null 2>&1; then
  echo "reusing healthy terminal-projection on http://127.0.0.1:8088" >&2
elif [ -x "$release_bin" ]; then
  "$release_bin" &
  gateway_pid=$!
  gateway_owned=1
elif command -v cargo >/dev/null 2>&1; then
  # Build/run the optimized profile (first run compiles once, then is cached).
  cargo run --release -p terminal-projection &
  gateway_pid=$!
  gateway_owned=1
elif [ -x "$debug_bin" ]; then
  # Last resort on a host without the Rust toolchain or a release artifact.
  # The debug binary is fine for short sessions but not long-running use.
  echo "warning: running the DEBUG gateway; rebuild with 'cargo build --release -p terminal-projection' for long sessions" >&2
  "$debug_bin" &
  gateway_pid=$!
  gateway_owned=1
else
  echo "terminal-projection requires cargo or a prebuilt target/*/terminal-projection" >&2
  exit 1
fi

cd "$terminal_root"
next_bin="$terminal_root/node_modules/.bin/next"
if [ ! -x "$next_bin" ]; then
  echo "terminal dashboard dependencies are missing; run npm ci --prefix terminal" >&2
  exit 1
fi
"$next_bin" dev
