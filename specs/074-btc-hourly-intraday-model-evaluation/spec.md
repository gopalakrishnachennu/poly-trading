# Phase 5.8 Specification: Frozen BTC-Hourly Intraday Model Evaluation

## Objective

Train and evaluate a deterministic intraday exit-return challenger on the
frozen 236-session supplemental dataset without allowing the model to trade.

## Scope

- Derive only causal current/past BBO, spread, depth, time, side, complementary
  price, and lagged-movement features.
- Train one deterministic LightGBM return regressor per predeclared horizon.
- Select exactly one horizon and prediction threshold on train/validation.
- Keep the chronological test fold unopened until selection is frozen.
- Simulate at most one non-overlapping position per contract using exact
  fixed-point labeled entry, exit, and fee-reserve P&L.
- Publish model text, policy/data/code digests, validation/test metrics, and a
  `REJECTED` or `PAPER_CANDIDATE_ONLY` verdict.

## Exclusions

- Online learning, test-set tuning, passive fill claims, full-depth claims,
  capital reservations, strategy activation, paper/live orders, credentials,
  wallets, or signatures.

## Acceptance criteria

- Fold digests must match the frozen dataset manifest.
- Features never read future labels or future frames.
- Selection fails closed when validation has too few trades or non-positive
  P&L. The test fold is then not evaluated.
- A test candidate requires positive net fixed-point P&L, minimum trade count
  and profit factor, and bounded drawdown. Passing means paper-candidate
  research only.
- Repeated runs with identical inputs and policy emit identical model and
  metric content except generation time.
