# ADR 0081: Classify executable positive exits separately from return size

- Status: Accepted
- Date: 2026-07-26

## Decision

After the loss-dominated return regressors failed development validation, one
separate challenger may classify whether a proposed taker entry has a positive
executable exit after the configured fee reserve. Candidate probability
thresholds are fixed before training and must satisfy per-fold activity and
profitability requirements.

## Consequences

Rare profitable paths are not erased by a median-loss regression objective.
Classification still grants no edge or authority by itself: exact integer P&L,
session walk-forward stability and the one-shot final fold remain binding.
