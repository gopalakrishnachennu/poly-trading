# Read-Only Terminal Projection Runbook

## Start

```text
cd terminal
npm install
npm run dev:full
```

Open `http://localhost:3000`. The launcher starts the Rust gateway on loopback
port 8088 and the terminal on port 3000. Stopping the launcher also stops the
gateway.

## Operator checks

- `curl http://127.0.0.1:8088/healthz` must report `no_trade: true`.
- A ready snapshot contains exactly BTC and ETH under
  `/api/v1/terminal/snapshot`.
- Every authority flag must be false.
- The terminal must always show `GLOBAL NO_TRADE` because this projection has no
  risk, accounting or execution authority.

## Expected fail-closed states

- `discovering`: no complete current two-asset snapshot has succeeded.
- `stale`: a prior success exists, but the latest atomic refresh failed.
- `halted`: clock or projection-owner integrity failed; restart only after the
  underlying clock/integrity cause is understood.

Missing/duplicate hourly markets, rollover gaps, one-sided or crossed books,
identity mismatch, invalid precision, upstream timeout, stale timestamps,
cross-book skew and partial BTC/ETH success all clear `assets`. The terminal
also clears its snapshot on HTTP, schema, authority-contract or freshness
failure. Do not relax these gates merely to keep prices visible.

## Authority boundary

This process reads public data only. It has no credentials, private keys,
wallet, user channel, accounting balances, risk approval, signatures, orders,
split/merge/redemption transactions or capital. Raw pair economics are not a
trade signal or locked profit.
