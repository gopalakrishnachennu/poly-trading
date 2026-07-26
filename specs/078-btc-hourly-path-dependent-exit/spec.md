# Phase 5.12 Specification: BTC-Hourly Path-Dependent Exit Challenger

## Objective

Test the intended intrahour strategy: enter only on an executable, liquid book,
then sell at the first executable take-profit, stop-loss or holding-time limit.

## Scope

- Apply immutable entry price, spread and session-time gates before modeling.
- Derive path-dependent exits from later executable bid frames in causal order.
- Charge the configured fee reserve and retain adverse price gaps exactly.
- Train a profitable-exit classifier using the causal v2 features.
- Select an absolute probability threshold with expanding, whole-session folds.
- Open the final chronological fold once only after development gates pass.

## Exclusions

- Mid-price exits, touched-price fills, look-ahead exits, random row splits,
  final-fold retuning, live activation, credentials, signing or orders.

## Acceptance criteria

- Each exit is the first qualifying future frame and occurs after entry.
- Missing or excessively delayed exit frames cannot become positive labels.
- Entry and exit economics use integer micros, including fee rounding.
- Session overlap, zero-activity folds and unstable development P&L reject the
  challenger before final evaluation.
