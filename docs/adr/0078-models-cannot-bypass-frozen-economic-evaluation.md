# ADR 0078: Models cannot bypass frozen economic evaluation

- Status: Accepted
- Date: 2026-07-26

## Decision

BTC-hourly intraday challengers train only on the frozen train fold, select
horizon and threshold only on validation, and evaluate the selected challenger
once on the chronological test fold. Backtests permit one non-overlapping
position per contract and use fixed-point executable entry/exit labels after
the configured fee reserve. Prediction accuracy alone is not a promotion gate.

## Consequences

The system can train advanced models without granting them capital or order
authority and without tuning on the answer key. A passing result is only a
paper-candidate artifact; the independent model-governance and core paper
boundaries remain mandatory.
