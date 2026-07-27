#!/usr/bin/env bash
# 1 Hz snapshot writer for Poly Advisor (Spec 094).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT/advisor"
if [[ ! -f dist/snapshot_daemon.js ]]; then
  npm install
  npm run build
fi
export POLY_REPO_ROOT="${POLY_REPO_ROOT:-$ROOT}"
export POLY_ADVISOR_API_URL="${POLY_ADVISOR_API_URL:-http://127.0.0.1:3000/api/btc-window-paper}"
export POLY_ADVISOR_SNAPSHOT_MS="${POLY_ADVISOR_SNAPSHOT_MS:-1000}"
exec node dist/snapshot_daemon.js
