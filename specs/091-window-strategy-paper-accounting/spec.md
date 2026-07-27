# Specification 091: Multi-Asset Window Paper Accounting and Attribution

## Objective

Extend `btc_window_engine_v1` from a decision-only tape into an auditable paper
book: every proposal carries its stake, every open position carries a
mark-to-market valuation, every close carries a realized result, and every
result is attributable to the setup that produced it. Widen coverage from BTC
only to every configured window asset so operators can compare venues.

## Scope

- Evaluate all configured window assets (BTC and ETH) across 5m/15m/1h.
- Carry the asset on the snapshot, the open position, and the journal record.
- Compute in fixed point, from observed executable quotes only:
  - `stake_micros` — collateral committed at entry (quantity x effective cost).
  - `mark_value_micros` — remaining quantity valued at the executable bid.
  - `unrealized_pnl_micros` — mark value minus remaining cost basis.
  - `realized_pnl_micros` — accumulated proceeds minus cost on reduced or
    closed quantity.
- Realize P&L on partial reductions at the executable bid, not the mid.
- Settle a window when it leaves the feed or its duration elapses, using the
  observed reference level against the recorded window open, and journal the
  binary outcome (`win` / `loss`) with the settled realized P&L.
- Maintain a runtime ledger aggregating realized, unrealized, wins, losses, and
  committed stake by setup, by asset, and by window.
- Publish the ledger in the worker status file and serve it to the terminal.
- Present the ledger in the terminal Positions workspace: portfolio ribbon,
  open positions with stake and P&L, strategy leaderboard, and a filterable
  decision tape.

## Exclusions

- Real-money submission, credentials, wallets, or signing.
- Mutating Paper Week campaign capital, pair journals, or risk reservations.
- Promotion of a SHADOW setup to `ACTIVE_CORE` on the strength of this ledger.
- Resolution via the official on-chain resolver; paper settlement uses the
  observed reference level and is labelled as such.

## Acceptance criteria

1. ETH 5m/15m/1h markets are evaluated and journalled with `asset="ETH"`; BTC
   behaviour is unchanged.
2. Every `Open` / `OpenReversed` record carries a non-zero `stake_micros` equal
   to quantity x effective entry cost in fixed point.
3. Every `Reduce` and `Exit` record carries `realized_pnl_micros` computed from
   the executable bid and the proportional cost basis.
4. A settled window emits one `Settle` record carrying `outcome` and the final
   `realized_pnl_micros`; a winning share settles at `1.0`, a losing share at
   `0.0`.
5. The ledger's realized total equals the sum of realized P&L across settled and
   closed records for the same journal.
6. Strategy attribution reports trades, wins, losses, and realized P&L per
   `setup_id`, and never merges `ACTIVE_CORE` with `SHADOW_EXPERIMENT` totals.
7. Journal records written before this spec still deserialize; new fields
   default rather than failing recovery.
8. No floating-point type appears in any stake, price, quantity, or P&L path.
9. Unit tests cover stake computation, partial realization, winning and losing
   settlement, ETH admission, and ledger aggregation.
