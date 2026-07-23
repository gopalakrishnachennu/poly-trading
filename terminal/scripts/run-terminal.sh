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
if command -v curl >/dev/null 2>&1 && curl -fsS --max-time 1 http://127.0.0.1:8088/healthz >/dev/null 2>&1; then
  echo "reusing healthy terminal-projection on http://127.0.0.1:8088" >&2
elif command -v cargo >/dev/null 2>&1; then
  cargo run -p terminal-projection &
  gateway_pid=$!
  gateway_owned=1
elif [ -x "$project_root/target/debug/terminal-projection" ]; then
  # Keep the local terminal usable on operator hosts that only have the
  # prebuilt, audited gateway artifact (for example, a workstation without
  # the Rust toolchain installed). The binary remains read-only and binds to
  # the same loopback endpoint as the source build.
  "$project_root/target/debug/terminal-projection" &
  gateway_pid=$!
  gateway_owned=1
else
  echo "terminal-projection requires cargo or target/debug/terminal-projection" >&2
  exit 1
fi

cd "$terminal_root"
vinext_bin="$terminal_root/node_modules/.bin/vinext"
if [ ! -x "$vinext_bin" ]; then
  echo "terminal dashboard dependencies are missing; run npm ci --prefix terminal" >&2
  exit 1
fi
WRANGLER_LOG_PATH=.wrangler/wrangler.log "$vinext_bin" dev
