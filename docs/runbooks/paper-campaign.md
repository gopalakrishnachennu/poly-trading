# One-Week Paper-Trading Campaign

This campaign uses public market data and a simulated ledger. It never loads
credentials, signs a request, accesses a wallet, or submits an order.

## What the week measures

- Net simulated P&L after modeled fees, slippage, latency, queue position, and
  partial fills.
- Realized, locked, and mark-to-market P&L; maximum drawdown and CVaR.
- Fill, hedge, cancel-race, rejection, and unknown-state rates.
- `NO_TRADE` rate and the reason for every rejected proposal.
- Feed completeness, sequence gaps, clock skew, stale intervals, and rollover
  correctness.
- Restart/checkpoint replay equality and journal corruption recovery.

Gross wins alone do not qualify a strategy. A campaign is valid only when its
raw inputs, decisions, simulated execution, ledger state, health evidence, and
final report replay deterministically.

## Durable record contract

Every record uses `paper-campaign-schema` and preserves:

```text
event_time_ns
recorded_time_ns
available_to_strategy_ns (in the decision record provenance)
campaign_id / stream / sequence
raw payload digest and decoded canonical fields
```

Record streams include market books, reference prices, strategy decisions,
paper execution lifecycle, reservations/accounting, settlement/resolution,
component health, checkpoints, and operator actions. The append-only journal is
the replay authority; analytics exports are derived and cannot create a
financial fact.

## Campaign gates

1. Start from a fresh campaign identifier and empty journal.
2. Verify public-feed eligibility and exact settlement-source configuration.
3. Set a fixed fake starting balance and immutable risk limits.
4. Capture raw events before strategy evaluation; never use corrected/final
   data that was unavailable at decision time.
5. Checkpoint at least hourly and after every restart or unknown state.
6. Stop and mark the campaign invalid on journal corruption, clock regression,
   unresolved unknown exposure, or conservation failure.
7. Produce daily rollups and a final report only after replaying the complete
   journal and matching every checkpoint digest.

The existing `integration-daemon` seven-day soak validates infrastructure and
rollover health. It is not a substitute for this paper P&L campaign until the
live public capture, paper-runtime wiring, and evidence report are connected.

## Local paper runner controls

The running read-only gateway exposes a loopback-only paper controller:

```text
GET  /api/v1/paper/status
POST /api/v1/paper/session
POST /api/v1/paper/session/stop
```

Start from the dashboard by entering principal and backup USD amounts and
pressing `START PAPER`. The dashboard sends fixed-point micros (never binary
floating point), starts separate BTC and ETH streams, and polls status every
second. `STOP PAPER` stops new simulation decisions while retaining the journal
and evidence already recorded.

The runner writes newline-delimited, digest-sealed evidence to
`POLY_PAPER_JOURNAL_DIR` (default `var/paper-campaign`). Each validated public
observation records both book sides, reference, target, feed age, decision and
reason. Paper executions record reservations, simulated pair fills, fees,
slippage, settlement handoffs and checkpoints. A complete-set opportunity is
simulated only when both executable asks plus modeled costs remain below the
configured payout; otherwise the authoritative decision is `NO_TRADE`.

This is intentionally not a live-trading switch: the API has no credentials,
wallet, signer, authenticated transport or order route, and the gateway remains
`127.0.0.1` by default.
