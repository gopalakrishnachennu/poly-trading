# Specification 089: BTC Multi-Window Strategy Engine

## Objective

Replace the retired directional and fixed-threshold paper strategies with one
fixed-point, every-second BTC UP/DOWN engine for 5-minute, 15-minute, and
1-hour markets. The engine maximises risk-adjusted expected value under
auditable paper controls: one combined decision per condition, ask-entry /
bid-exit valuation, universal exits/hedges/reversals, and separated performance
books. No real-money authority is granted.

## Scope

- Deterministic every-second evaluation for BTC 5m, 15m, and 1h windows.
- Shared feature construction, calibrated edge, sizing, and risk pauses.
- Position state machine: FLAT, CANDIDATE, OPEN, WARNING, PROTECT,
  REVERSE_CANDIDATE, HEDGED, LOCKED.
- Core candidates for 5m and 15m; 1h directional and sequential-hedge paths in
  SHADOW / EXPERIMENTAL mode.
- Complete-set and sequential-lock scanners with orphan-leg protection and no
  forced trades.
- Immutable strategy version IDs and journal fields for identity, signal,
  price, position, actions, exit, and data quality.
- Separate books: ACTIVE_CORE, SHADOW_EXPERIMENT, RETIRED_LEGACY,
  INVALID_INCOMPLETE.
- Explicit DISABLE of legacy book-imbalance, favourite p>0.60, old fair-value,
  multi-momentum, and fixed-threshold cheap-token round-trip as order triggers.

## Exclusions

- Real-money trading, wallets, credentials, signing, or live submission.
- ETH windows (BTC-only for this specification).
- Averaging down, midpoint fills, or one-tick automatic reversals.
- Mixing RETIRED_LEGACY simulator statistics into ACTIVE_CORE performance.
- Promotion past paper until Section 12 validation gates are met.

## Acceptance criteria

1. All prices, fees, quantities, edges, stops, and P&L use fixed-point integer
   micros; binary floating point is forbidden for money fields.
2. Entry valuation uses ask; exit valuation uses bid; stale or one-sided books
   produce `NO_TRADE` with an attributable rejection reason.
3. At most one directional exposure and at most one reversal exist per
   `condition_id`.
4. Open-position management (lock, protect, reduce, exit, reverse-candidate)
   runs before any new-entry evaluation on that second.
5. 5m core entry rules enforce persistence, confidence, ask band, spread, BTC
   confirmation, non-negative short momentum, and minimum edge 0.04.
6. 15m core/acceleration/continuation setups enforce their phase windows,
   persistence, confidence, ask caps, spread, BTC confirmation, and minimum
   edge 0.05.
7. 1h minute-50 favourite and sequential-hedge paths journal under
   `SHADOW_EXPERIMENT` and cannot mutate ACTIVE_CORE performance.
8. Complete-set and sequential locks require fresh two-sided depth, total
   effective cost thresholds, and never force a trade when the lock is absent.
9. Every decision journals strategy version, rejection/entry reasons, and
   performance book class.
10. Legacy strategies listed in Section 11 remain DISABLED as order triggers.
11. The crate proposes paper intents only; it cannot reserve capital, approve
    risk, sign, or submit orders.
12. Unit tests cover happy path, data-quality reject, emergency stop, reverse
    confirmation, sequential lock, high-ask reject, and legacy-disabled markers.
