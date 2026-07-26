# Phase 5.10 Specification: BTC-Hourly Walk-Forward Challenger

## Objective

Train one deterministic BTC-hourly intraday challenger with causal order-book,
Binance spot and verified Polymarket flow features, then evaluate it on a
session-separated final fold exactly once.

## Scope

- Bind the frozen research dataset, auxiliary manifest and typed model policy by
  checksum.
- Reserve the final chronological sessions before any model or threshold
  selection.
- Select only from a predeclared model, horizon and threshold grid using
  expanding-window, whole-session validation folds.
- Preserve the configured historical-source latency reserve when joining spot
  and flow evidence.
- Evaluate fills and P&L with the existing conservative executable labels and
  integer-micro accounting.
- Publish fold metrics, final metrics, feature names, checksums and a fail-closed
  research verdict.

## Exclusions

- Random row splits, test-fold retuning, synthetic fills, guaranteed-profit
  claims, strategy activation, capital reservations, signing or order
  submission.

## Acceptance criteria

- No feature timestamp can exceed the decision's causal source cutoff.
- Train, validation and final-test sessions are pairwise disjoint.
- A candidate must pass aggregate and per-fold development gates before the
  final fold is read.
- The final fold is evaluated at most once with a frozen objective, horizon and
  threshold.
- Exact P&L, fees and drawdown remain integer micros.
- Failed development or final gates produce a rejected verdict and no
  activation artifact.
