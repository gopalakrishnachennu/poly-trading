#!/usr/bin/env bash
# Start Poly Advisor MCP over stdio (Spec 094).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT/advisor"
if [[ ! -f dist/index.js ]]; then
  npm install
  npm run build
fi
export POLY_REPO_ROOT="${POLY_REPO_ROOT:-$ROOT}"
export POLY_ADVISOR_API_URL="${POLY_ADVISOR_API_URL:-http://127.0.0.1:3000/api/btc-window-paper}"
exec node dist/index.js
