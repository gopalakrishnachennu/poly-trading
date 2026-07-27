#!/usr/bin/env bash
# Streamable HTTP MCP for ChatGPT Pro (Spec 094).
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
cd "$ROOT/advisor"
if [[ ! -f dist/http_server.js ]]; then
  npm install
  npm run build
fi
export POLY_REPO_ROOT="${POLY_REPO_ROOT:-$ROOT}"
export POLY_ADVISOR_API_URL="${POLY_ADVISOR_API_URL:-http://127.0.0.1:3000/api/btc-window-paper}"
export POLY_ADVISOR_HTTP_HOST="${POLY_ADVISOR_HTTP_HOST:-127.0.0.1}"
export POLY_ADVISOR_HTTP_PORT="${POLY_ADVISOR_HTTP_PORT:-8787}"
# Stable token file so restarts keep the same Bearer secret
TOKEN_FILE="${POLY_ADVISOR_TOKEN_FILE:-$ROOT/var/poly-advisor/http.token}"
mkdir -p "$(dirname "$TOKEN_FILE")"
if [[ -z "${POLY_ADVISOR_HTTP_TOKEN:-}" ]]; then
  if [[ -f "$TOKEN_FILE" ]]; then
    export POLY_ADVISOR_HTTP_TOKEN="$(cat "$TOKEN_FILE")"
  else
    export POLY_ADVISOR_HTTP_TOKEN="$(openssl rand -hex 24)"
    printf '%s' "$POLY_ADVISOR_HTTP_TOKEN" > "$TOKEN_FILE"
    chmod 600 "$TOKEN_FILE"
  fi
fi
echo "poly-advisor-http: token file $TOKEN_FILE" >&2
exec node dist/http_server.js
