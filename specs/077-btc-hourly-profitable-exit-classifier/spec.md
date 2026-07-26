# Phase 5.11 Specification: BTC-Hourly Profitable-Exit Classifier

## Objective

Determine whether the rare positive round-trip labels in the BTC-hourly
development sessions are causally distinguishable without retuning against the
reserved final sessions.

## Scope

- Reuse the checksum-verified v2 causal feature engine and session partition.
- Label a row positive only when exact exit proceeds exceed executable entry
  cost and the configured fee reserve.
- Select a horizon and absolute probability threshold across expanding,
  whole-session folds.
- Require minimum activity in every fold, positive aggregate P&L and positive
  P&L in a configured majority of folds.
- Freeze and evaluate the final chronological sessions once only after all
  development gates pass.

## Exclusions

- Class labels based on mark prices, random row splits, synthetic oversampling,
  final-fold tuning, activation, capital, credentials, signing or orders.

## Acceptance criteria

- Positive labels and reported P&L use exact integer-micro economics.
- All causal feature and fold invariants from Phase 5.10 remain enforced.
- Zero-trade folds cannot satisfy the activity gate.
- A failed classifier publishes a rejected evidence report, not a model
  promotion.
