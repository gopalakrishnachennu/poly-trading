# Phase 3.5 Specification: Live-Data Paper-Trading Composition

## Objective

Certify deterministic paper-trading decisions and accounting from captured
market data while preserving point-in-time availability, conservative execution
assumptions, walk-forward separation and zero external mutation.

## Scope

- Exact current Phase 3.4 local campaign report binding.
- Immutable captured-session manifests and ordered event records.
- Separate event, receive and strategy-available timestamps.
- Optimistic, estimated and conservative queue-position cases.
- Bounded signal, submission, acknowledgement and cancellation latency.
- Zero, partial, full, unknown and cancel-race paper execution outcomes.
- Exact existing proposal, risk, reservation, paper execution, settlement and
  accounting evidence digests.
- Chronological train, validation and test folds with frozen strategy identity.
- Journal replay, checkpoints and create-new certification reports.

## Exclusions

- External feeds during certification, credentials, signing or submission
- Crediting fills from price touch alone or assuming simultaneous pair fills
- Features whose available time is after the decision time
- Real profit, capital, live trading or deployment authority

## Acceptance criteria

- Stale, substituted, live-certified or authority-bearing Phase 3.4 evidence
  fails registration.
- Captured records are unique, contiguous, provenance-bound and ordered by
  strategy-available time without changing source event/receive time.
- No decision consumes a record after its decision cutoff or from validation or
  test data during training.
- Every candidate is evaluated under all queue cases, configured latency and
  zero/partial/full/unknown/cancel-race outcomes.
- Conservative evidence cannot be replaced by optimistic evidence and a price
  touch alone never proves a passive fill.
- Accounting and settlement evidence bind exact existing paper-runtime outputs;
  the campaign cannot inject a fill, posting or confirmed balance.
- Walk-forward folds are chronological, non-overlapping and bind one frozen
  strategy digest before the final test fold.
- Final reports grant no credential, connection, mutation, deployment, capital,
  trading or submission authority and mark real P&L false.
- Tests and TLA+ cover availability time, fold separation, conservative fills,
  unknown retention, no mutation and absorbing halt.
