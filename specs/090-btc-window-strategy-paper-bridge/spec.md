# Specification 090: BTC Window Strategy Managed Paper Bridge

## Objective

Run `btc_window_engine_v1` continuously against live BTC 5m/15m/1h observation
feeds in paper mode under the managed terminal stack, without granting live
order, wallet, signing, or Paper Week capital authority.

## Scope

- Poll the unified windows observation feed (or local capture + gateway
  fallback) once per second for BTC markets only.
- Convert executable bid/ask, sizes, timestamps, and BTC reference levels into
  fixed-point `MarketSecondSnapshot` values.
- Evaluate `EngineState::on_every_second` per `condition_id`.
- Append decision journals under `var/btc-window-paper/` with strategy version,
  performance book, and `real_money=false`.
- Publish a non-authoritative status file for operators.
- Supervise the worker from `scripts/run-terminal-stack.sh` after the terminal
  is healthy.
- Keep legacy Python directional/intraday workers disabled.

## Exclusions

- Real-money submission, credentials, wallets, or signing.
- Mutating Paper Week campaign capital, pair journals, or risk reservations.
- ETH markets in this bridge.
- Automatic promotion from SHADOW 1h to ACTIVE_CORE.

## Acceptance criteria

1. The managed stack starts the BTC window paper worker without starting legacy
   Python simulators.
2. Every journalled decision includes `strategy_version=btc_window_engine_v1`,
   `paper_only=true`, and `real_money=false`.
3. Snapshot conversion rejects midpoint-only fills and missing bid/ask sides.
4. Open-position management still precedes new entries (engine invariant).
5. Worker status reports last tick time, active conditions, and last action
   without claiming live-trading authority.
6. Unit tests cover feed parsing and one end-to-end paper tick over a fixture
   windows payload.
