# Phase 2.7 Specification: Deterministic Complete-Set Arbitrage Detection

## Objective

Detect conservative top-of-book buy-pair and sell-pair opportunities from an
immutable Phase 2.6 strategy context and emit only a paired set of inert
proposal intents.

## Requirements

1. Every evaluation binds an exact checksummed Phase 2.6 context and its
   complementary Up/Down token identities.
2. Opportunity output requires a current `ACTIVE_READY`, unexpired context and
   authoritative two-sided books for both tokens.
3. Executable quantity is the minimum of both selected top-level quantities and
   the configured maximum, and must meet the configured minimum. The modeled
   partial-fill quantity is strictly below the minimum executable quantity.
4. Buy-pair cost rounds each leg upward. Sell-pair proceeds round each leg
   downward. Maximum per-leg fees and conversion cost are explicit inputs.
5. Buy-pair profit is pair payout minus both costs, fees, and conversion cost.
   Sell-pair profit is both proceeds minus split collateral, fees, and
   conversion cost.
6. Net profit and ROI thresholds are inclusive, integer-only, and checked.
7. A detected plan contains exactly two Phase 2.6 `ProposalIntent` values with
   derived non-substitutable proposal identities and exact context expiry.
8. A detected plan is an opportunity, not locked profit. It grants no risk,
   capital, merge/split, signing, or execution authority.
9. Invalid/degraded/expired/illiquid/unprofitable cases are attributable normal
   no-opportunity decisions. Digest/history/idempotency/arithmetic or durable
   corruption is an absorbing halt.
10. Commands are bounded, canonical, content-idempotent, journal-first,
    replayable, checkpoint-verifiable, and digest-stable.

## Acceptance criteria

- Tests cover exact buy and sell economics, conservative rounding, threshold
  boundaries, liquidity bounds, degraded and expired state, derived identities,
  idempotency/history failures, sync failure, replay/checkpoint equality, and
  monotonic fee/profit properties.
- A bounded TLA+ model proves two-leg output, readiness, profitability,
  no-authority, one-use evaluation, and absorbing halt invariants.
- Formatting, warnings-denied Clippy, all workspace tests, and all formal models
  pass.

## Exclusions

Depth sweeping, queue/fill prediction, cross-market arbitrage, market making,
directional models, capital reservation, split/merge transactions, credentials,
signing, authenticated transport, and live submission.
