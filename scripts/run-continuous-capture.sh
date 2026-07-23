#!/usr/bin/env sh
set -eu

# Unattended, supervised research capture: keeps the read-only terminal-projection
# gateway and the compact snapshot recorder alive for long (weeks-scale) runs.
# Both children are auto-restarted with exponential backoff; a status heartbeat
# and logs are written under var/. Everything here is read-only and credentialless.
#
#   scripts/run-continuous-capture.sh
#
# Stop with Ctrl-C (or SIGTERM); both children are stopped cleanly.

project_root=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
cd "$project_root"

log_dir="var/log"
status_file="var/capture-status.txt"
mkdir -p "$log_dir" var/research-capture

# File-bound gateway configuration, read once by the gateway (same contract as
# the terminal launcher). An absent policy must keep paper execution blocked.
if [ -f "$project_root/config/terminal-runtime.json" ]; then
  export POLY_TERMINAL_CONFIG_PATH="$project_root/config/terminal-runtime.json"
fi
if [ -f "$project_root/config/paper-market-policy.json" ]; then
  export POLY_PAPER_POLICY_PATH="$project_root/config/paper-market-policy.json"
fi

interval="${POLY_CAPTURE_INTERVAL:-15}"

gateway_pid=""
recorder_pid=""

status() {
  printf '%s  gateway_pid=%s recorder_pid=%s interval=%ss\n' \
    "$(date -u +%Y-%m-%dT%H:%M:%SZ)" "${gateway_pid:-none}" "${recorder_pid:-none}" "$interval" \
    > "$status_file"
}

cleanup() {
  echo "stopping capture..." >&2
  kill "${recorder_pid:-}" "${gateway_pid:-}" 2>/dev/null || true
  wait 2>/dev/null || true
  exit 0
}
trap cleanup INT TERM

gateway_healthy() {
  command -v curl >/dev/null 2>&1 &&
    curl -fsS --max-time 2 http://127.0.0.1:8088/healthz >/dev/null 2>&1
}

# Supervise one command: restart with exponential backoff (capped) on exit.
# $1 = label, $2 = log file, rest = command.
supervise() {
  label="$1"; logf="$2"; shift 2
  backoff=1
  while :; do
    echo "$(date -u +%FT%TZ) [$label] starting: $*" >> "$logf"
    "$@" >> "$logf" 2>&1 &
    child=$!
    # publish which pid we started
    if [ "$label" = gateway ]; then gateway_pid=$child; else recorder_pid=$child; fi
    status
    wait "$child" 2>/dev/null || true
    code=$?
    echo "$(date -u +%FT%TZ) [$label] exited ($code); restarting in ${backoff}s" >> "$logf"
    sleep "$backoff"
    backoff=$(( backoff * 2 )); [ "$backoff" -gt 60 ] && backoff=60
  done
}

# 1) Gateway: reuse an already-healthy one, else supervise our own.
if gateway_healthy; then
  echo "reusing healthy gateway on http://127.0.0.1:8088" >&2
elif command -v cargo >/dev/null 2>&1; then
  supervise gateway "$log_dir/gateway.log" cargo run --locked -p terminal-projection &
elif [ -x "$project_root/target/debug/terminal-projection" ]; then
  supervise gateway "$log_dir/gateway.log" "$project_root/target/debug/terminal-projection" &
else
  echo "need cargo or target/debug/terminal-projection to run the gateway" >&2
  exit 1
fi

# 2) Wait for the gateway to answer before starting the recorder.
tries=0
until gateway_healthy; do
  tries=$(( tries + 1 ))
  [ "$tries" -gt 120 ] && { echo "gateway did not become healthy" >&2; cleanup; }
  sleep 1
done
echo "gateway healthy; starting snapshot recorder (interval ${interval}s)" >&2

# 3) Recorder: supervise the compact snapshot poller.
supervise recorder "$log_dir/recorder.log" \
  python3 scripts/capture_snapshots.py --interval "$interval" &

status
# Keep the supervisor foregrounded so trap handling works.
wait
