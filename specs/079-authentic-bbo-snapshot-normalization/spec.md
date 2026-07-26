# Phase 5.13 Specification: Authentic BBO Snapshot Normalization

## Objective

Replace synthetic within-second bid/ask extrema with one deterministic,
authentic usable source snapshot per verified market, token and second.

## Scope

- Preserve the existing pinned source, official identity and resolution gates.
- Select the latest usable source timestamp in each one-second bucket.
- Resolve exact-timestamp duplicates by selecting one complete source row; never
  combine prices or sizes from different rows.
- Preserve the configured source-only latency reserve and fixed-point rounding.
- Publish a create-new tape and checksum manifest with an explicit compaction
  mode.

## Exclusions

- Claiming unavailable historical receive time, passive fill simulation,
  full-depth reconstruction, model promotion, paper activation or live orders.

## Acceptance criteria

- The selected bid, ask and sizes all originate from one source row.
- Unsorted Parquet row groups produce globally chronological deterministic
  output.
- Identity, outcome, crossed-book, empty-size, bounds and checksum failures
  remain fail-closed.
- Existing conservative-extrema evidence remains reproducible and untouched.
