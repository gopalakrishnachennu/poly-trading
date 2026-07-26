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

# New capture sessions use bounded, checksummed segment directories. Existing
# single-file journals are intentionally left untouched: do not rename,
# truncate, migrate, or delete them while investigating or retaining history.
market_segments="$tick_dir/public-clob-segments"
reference_segments="$tick_dir/reference-market-segments"

echo "starting segmented public capture: $market_segments" >&2
echo "starting segmented reference capture: $reference_segments" >&2

cleanup() {
  kill "${market_pid:-}" "${reference_pid:-}" 2>/dev/null || true
}
trap cleanup INT TERM EXIT

cargo run --locked -p public-market-data -- --segments "$market_segments" &
market_pid=$!
cargo run --locked -p reference-market-data -- --segments "$reference_segments" &
reference_pid=$!

wait "$market_pid" "$reference_pid"
