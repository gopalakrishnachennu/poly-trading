# Phase 4.1 Specification: Offline Model Governance and Shadow Evaluation

## Objective

Introduce a deterministic, paper-only governance boundary for adaptive research
models. It accepts immutable model artifacts and evaluation evidence, compares a
champion with challengers, and emits only a non-authorizing promotion decision.

## Scope

- Immutable model identity, feature schema and training-data provenance.
- Chronological train/validation/test boundaries with a frozen final test.
- Separate research, evaluation and adversarial-review roles.
- Net P&L, drawdown, CVaR, fees, slippage, fill quality, data coverage and
  hedge-failure evaluation metrics represented as fixed-point integers.
- Champion/challenger decisions with explicit regression, diversity and drift
  gates.
- Proposal-only output with a strict `NO_TRADE` fallback.
- Deterministic state digest, input validation and property tests.

## Exclusions

- Online parameter updates, autonomous model promotion, credentials, signing,
  wallet access, order submission, live trading and capital authority.
- Claiming that a paper evaluation guarantees profitability.

## Acceptance criteria

- Invalid, stale, duplicated, substituted, future-dated or overlapping
  evidence is rejected.
- A model cannot train on validation/test data or evaluate an unfrozen model on
  the final test fold.
- A challenger cannot replace a champion unless all required gates pass on
  unseen test evidence after fees, slippage and conservative execution costs.
- Any missing, stale, drifted, unreconciled or adverse-review failure emits
  `NO_TRADE` and grants no authority.
- Every decision binds immutable model, feature, data, policy and evidence
  digests.
- Unit and property tests cover leakage, substitution, regression, drift,
  duplicate identities, role conflicts, arithmetic bounds and no-authority
  behavior.
