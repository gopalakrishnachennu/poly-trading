#!/usr/bin/env sh
set -eu

# Credentialless, journal-first tick capture for the current hourly BTC/ETH
# prediction markets and their Binance reference streams. It contains no
# authenticated venue client, wallet, signer, order, or cancellation path.

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is required to run tick capture" >&2
  exit 1
fi

tick_dir=${POLY_TICK_JOURNAL_DIR:-var/tick-capture}
mkdir -p "$tick_dir"

market_journal="$tick_dir/public-clob.journal"
reference_journal="$tick_dir/reference-market.journal"

cleanup() {
  kill "${market_pid:-}" "${reference_pid:-}" 2>/dev/null || true
}
trap cleanup INT TERM EXIT

cargo run --locked -p public-market-data -- "$market_journal" &
market_pid=$!
cargo run --locked -p reference-market-data -- "$reference_journal" &
reference_pid=$!

wait "$market_pid" "$reference_pid"
