#!/usr/bin/env sh
set -eu

# Unattended supervisor for the full Poly-Trading dashboard stack: keeps the
# gateway, the multi-window capture, and the Next.js terminal alive together
# with exponential-backoff restart. Everything is read-only and credentialless.
#
# Start (detached, survives shell exit):
#   nohup scripts/run-terminal-stack.sh > var/log/stack-supervisor.log 2>&1 &
#
# Stop:
#   scripts/run-terminal-stack.sh stop
#
# Status:
#   scripts/run-terminal-stack.sh status
#
# One process supervises gateway, capture, reference snapshot, terminal and
# rolling research children. On SIGTERM/SIGINT they are stopped cleanly.
# Ports: gateway 8088, terminal 3000.

project_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$project_root"

log_dir="var/log"
pid_file="var/stack-supervisor.pid"
status_file="var/stack-status.txt"
mkdir -p "$log_dir" var

if [ -f "$project_root/config/terminal-runtime.json" ]; then
  export POLY_TERMINAL_CONFIG_PATH="$project_root/config/terminal-runtime.json"
fi
if [ -f "$project_root/config/paper-market-policy.json" ]; then
  export POLY_PAPER_POLICY_PATH="$project_root/config/paper-market-policy.json"
fi
# The fixed-threshold intraday challenger is retired from the managed default.
# It has negative observed paper P&L and is historical research evidence only.
# A separately governed experiment needs both values; a plain `=1` cannot
# accidentally revive it after a restart or inherited shell environment.
: "${POLY_ENABLE_UNVALIDATED_INTRADAY:=0}"
: "${POLY_INTRADAY_EXPERIMENT_ACK:=}"

gateway_healthy() {
  curl -fsS --max-time 2 http://127.0.0.1:8088/healthz >/dev/null 2>&1
}
terminal_healthy() {
  curl -fsS --max-time 2 http://127.0.0.1:3000/api/windows >/dev/null 2>&1
}

if [ "${1:-run}" = "stop" ]; then
  # Kill every `supervise` shell loop, not just the recorded main pid.
  # Each `supervise` call backgrounds a `while :` shell that keeps a child
  # alive; if we only kill the main pid, those loop shells become orphans
  # (re-parented to init) and keep respawning their children. Result of the
  # old code: repeated stop/start cycles accumulated duplicate captures and
  # traders that raced on shared state files. Fix: pkill the supervisor
  # script by name so every instance (main + supervise loops) dies together.
  pkill -f "run-terminal-stack.sh" 2>/dev/null || true
  # Then belt-and-braces the children on the well-known ports and names.
  lsof -ti tcp:8088 -sTCP:LISTEN 2>/dev/null | xargs -r kill 2>/dev/null || true
  lsof -ti tcp:3000 -sTCP:LISTEN 2>/dev/null | xargs -r kill 2>/dev/null || true
  pkill -f capture_windows.py 2>/dev/null || true
  pkill -f capture_snapshots.py 2>/dev/null || true
  pkill -f live_paper_trader.py 2>/dev/null || true
  pkill -f live_paper_intraday.py 2>/dev/null || true
  pkill -f "profit_engine.run_pipeline" 2>/dev/null || true
  pkill -f run_btc_hourly_research_cycle.py 2>/dev/null || true
  rm -f "$pid_file"
  echo "stopped." >&2
  exit 0
fi

if [ "${1:-run}" = "status" ]; then
  echo "supervisor: $( { [ -f "$pid_file" ] && kill -0 "$(cat "$pid_file")" 2>/dev/null && echo "up pid=$(cat "$pid_file")"; } || echo "DOWN")"
  echo "gateway   : $(gateway_healthy && echo "up" || echo "DOWN")"
  echo "terminal  : $(terminal_healthy && echo "up" || echo "DOWN")"
  echo "capture   : $(pgrep -f capture_windows.py >/dev/null && echo "up pid=$(pgrep -f capture_windows.py)" || echo "DOWN")"
  echo "snapshot  : $(pgrep -f capture_snapshots.py >/dev/null && echo "up pid=$(pgrep -f capture_snapshots.py)" || echo "DOWN")"
  echo "legacy paper / intraday: RETIRED (core Paper Week only)"
  echo "research  : $(pgrep -f run_btc_hourly_research_cycle.py >/dev/null && echo "up pid=$(pgrep -f run_btc_hourly_research_cycle.py)" || echo "DOWN")"
  echo "runtime state: derived from current process identities; legacy status files are ignored"
  exit 0
fi

# Refuse to double-start.
if [ -f "$pid_file" ] && kill -0 "$(cat "$pid_file")" 2>/dev/null; then
  echo "supervisor already running pid=$(cat "$pid_file")" >&2
  exit 1
fi
echo $$ > "$pid_file"

gateway_pid=""; recorder_pid=""; snapshot_pid=""; terminal_pid=""; research_pid=""

pid_for() {
  pgrep -f "$1" 2>/dev/null | head -n 1 || true
}

status() {
  printf '%s  gateway_pid=%s recorder_pid=%s snapshot_pid=%s terminal_pid=%s legacy_trader=RETIRED intraday=RETIRED research_pid=%s\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" \
    "$(pid_for 'terminal-projection')" "$(pid_for 'capture_windows.py')" "$(pid_for 'capture_snapshots.py')" "$(pid_for 'next dev')" "$(pid_for 'run_btc_hourly_research_cycle.py')" \
    > "$status_file"
}

cleanup() {
  echo "stopping stack..." >&2
  kill "${research_pid:-}" "${terminal_pid:-}" "${snapshot_pid:-}" "${recorder_pid:-}" "${gateway_pid:-}" 2>/dev/null || true
  # kill their process groups too (Next.js spawns children)
  for pid in "${research_pid:-}" "${terminal_pid:-}" "${snapshot_pid:-}" "${recorder_pid:-}" "${gateway_pid:-}"; do
    [ -n "$pid" ] && pkill -P "$pid" 2>/dev/null || true
  done
  rm -f "$pid_file"
  exit 0
}
trap cleanup INT TERM

# Supervise one command: restart with exponential backoff (capped) on exit.
supervise() {
  label="$1"; logf="$2"; shift 2
  backoff=1
  while :; do
    echo "$(date -u +%FT%TZ) [$label] starting: $*" >> "$logf"
    "$@" >> "$logf" 2>&1 &
    child=$!
    case "$label" in
      gateway)  gateway_pid=$child;;
      recorder) recorder_pid=$child;;
      snapshot) snapshot_pid=$child;;
      terminal) terminal_pid=$child;;
      research) research_pid=$child;;
    esac
    status
    wait "$child" 2>/dev/null || true
    code=$?
    echo "$(date -u +%FT%TZ) [$label] exited ($code); restarting in ${backoff}s" >> "$logf"
    sleep "$backoff"
    backoff=$(( backoff * 2 )); [ "$backoff" -gt 60 ] && backoff=60
  done
}

# 1) Gateway.
release_bin="$project_root/target/release/terminal-projection"
if gateway_healthy; then
  echo "reusing healthy gateway on http://127.0.0.1:8088" >&2
elif [ -x "$release_bin" ]; then
  supervise gateway "$log_dir/gateway.log" "$release_bin" &
elif command -v cargo >/dev/null 2>&1; then
  supervise gateway "$log_dir/gateway.log" cargo run --release --locked -p terminal-projection &
else
  echo "need target/release/terminal-projection or cargo" >&2
  cleanup
fi

# 2) Wait for the gateway.
tries=0
until gateway_healthy; do
  tries=$(( tries + 1 ))
  [ "$tries" -gt 120 ] && { echo "gateway never became healthy" >&2; cleanup; }
  sleep 1
done
echo "gateway healthy" >&2

# 3) Multi-window capture (BTC+ETH 5m/15m; feeds var/windows-latest.json).
supervise recorder "$log_dir/capture-windows.log" \
  python3 scripts/capture_windows.py &

# 4) Terminal (Next.js dev server on :3000).
supervise terminal "$log_dir/terminal.log" \
  sh -c 'cd terminal && npm run dev' &

# 5) Wait for the terminal.
tries=0
until terminal_healthy; do
  tries=$(( tries + 1 ))
  [ "$tries" -gt 120 ] && { echo "terminal never became healthy" >&2; break; }
  sleep 1
done
echo "terminal healthy on http://localhost:3000" >&2

# 6) Legacy directional and fixed-threshold intraday simulators are retired.
# They bypass the core-backed fixed-point Paper Week campaign and must never be
# launched by this managed stack. Historical evidence files remain untouched.
echo "legacy directional simulator disabled: RETIRED_LEGACY_BYPASS" >&2
echo "intraday challenger disabled: RETIRED_NEGATIVE_OOS" >&2

# 7) Compact causal reference snapshots used by the accepted fixed-point
# research normalizer. This is read-only public data and carries no authority.
supervise snapshot "$log_dir/capture-snapshots.log" \
  python3 scripts/capture_snapshots.py --interval 15 &

# 8) Immutable fixed-point rolling research. A passing model may be added to
# the paper-only registry, but this process has no paper execution or live
# order capability. The configured process lock prevents duplicate loops.
supervise research "$log_dir/btc-hourly-research-cycle.log" \
  python3 scripts/run_btc_hourly_research_cycle.py \
    --config config/btc-hourly-rolling-research.example.json --loop &

status
wait
