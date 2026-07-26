# Phase 5.19 Specification: BTC Five-Minute Per-Second Export

## Objective

Convert the complete locally captured BTC five-minute Polymarket REST book
evidence into an immutable, human-readable dataset containing exactly 300
timestamped rows for every authentic five-minute contract.

## Scope

- Verify immutable BTC market identity, distinct Up/Down tokens, exact
  five-minute UTC boundaries and fixed-point book values.
- Merge restart duplicates without merging conflicting identities or book
  observations.
- Preserve the collector's batch-start timestamp and derive a conservative
  strategy-availability timestamp from versioned policy.
- Emit one row for every second, including explicit missing and stale states.
- Carry the last authentic book only with its observation time, availability
  time and age; never label a carried value as a fresh observation.
- Add causal predictive BTC reference values when locally captured, while
  keeping reference absence independent from Polymarket book availability.
- Partition CSV output by UTC date/hour and publish checksummed manifests plus
  a content-addressed dataset revision.

## Exclusions

- Fabricated ticks, interpolation, backward fill, silent forward fill, trade
  labels, resolution labels, model training, paper activation, live trading,
  credentials, signing, wallet access or order submission.

## Acceptance criteria

- Every accepted market emits exactly 300 unique second rows.
- A book can appear in a row only after its conservative availability time.
- Missing, one-sided, stale and out-of-window evidence is attributable.
  Crossed, malformed and unknown-token evidence fails the export; a condition
  containing same-time book equivocation is quarantined in full and listed in
  the manifest.
- Prices and quantities remain integer fixed-point values.
- Repeated identical inputs and policy produce the same dataset revision.
- Every output CSV and source file is checksum-bound by the manifest.
