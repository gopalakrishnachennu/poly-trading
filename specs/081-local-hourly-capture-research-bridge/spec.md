# Phase 5.15 Specification: Local Hourly Capture Research Bridge

## Objective

Convert immutable local BTC one-hour book polling and reference observations
into the same causal, fixed-point research contracts used by the historical
walk-forward pipeline.

## Scope

- Accept only checksum-bound, size-bounded source files.
- Treat the hourly book timestamp as local observation time because the
  recorder assigns it immediately before reading the validated gateway frame.
- Require an atomic UP/DOWN book pair for a condition and observation time.
- Validate immutable identity, token mapping, one-hour boundaries, sorted
  depth, price and quantity bounds independently of the gateway.
- Deduplicate exact restart overlap and reject conflicting duplicates.
- Export fixed-point Binance reference observations with their original local
  event time.
- Merge non-overlapping historical and local evidence into create-new,
  checksummed tape and auxiliary artifacts.

## Exclusions

- Reconstructing unavailable exchange event time, queue priority or trade flow.
- Weakening model-promotion thresholds, opening a frozen final test after a
  failed development gate, activating paper strategy, or placing live orders.

## Acceptance criteria

- Every emitted local book has observed receive time and explicit
  `LOCAL_GATEWAY_POLL_OBSERVATION` provenance.
- No one-leg or cross-condition frame is emitted.
- Corrupt JSON, changed identity, crossed/unsorted depth, checksum mismatch,
  source growth beyond policy, or chronological overlap rejects the artifact.
- Historical inputs remain immutable and independently checksum verified.
- Resulting model evaluation remains walk-forward by independent session and
  reports rejection honestly when no repeatable net edge exists.
