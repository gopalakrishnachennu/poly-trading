# Phase 5.16 Specification: Path Net-Value Challenger

## Objective

Predict the exact executable net P&L of a predeclared intrahour exit path,
rather than optimizing only the probability that the path is positive.

## Scope

- Preserve the frozen entry, take-profit, stop-loss, timeout, fee, spread,
  slippage, latency and non-overlap rules.
- Use only features consistently available in historical BBO and local
  full-depth captures: top-of-book, time, pair price and causal Binance spot.
- Exclude depth and Polymarket trade-flow features because the final local
  sessions do not contain equivalent evidence.
- Select objective and minimum predicted net value only on chronological
  walk-forward development folds.
- Keep the final 24 independent sessions sealed unless every development gate
  passes.

## Acceptance criteria

- Labels and simulated P&L remain integer micros.
- Model inputs never include future data or settlement outcome.
- A high win probability cannot promote a strategy with negative expected
  dollars.
- Missing folds, insufficient trades, unstable fold profitability, negative
  aggregate P&L or an unseen-test failure reject the challenger.
- A passing result remains paper-candidate-only and grants no order authority.
