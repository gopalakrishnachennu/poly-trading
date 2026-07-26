# Phase 5.20 Specification: BTC Five-Minute Single-File Merge

## Objective

Create one human-readable CSV from every partition in a sealed BTC five-minute
per-second export without weakening its identity, timing, fixed-point, quality
or checksum guarantees.

## Scope

- Resolve one checksum-bound source revision through its exact latest pointer.
- Verify the root manifest, every partition manifest and every partition CSV.
- Preserve the canonical column order and source row values at the CSV-field
  level.
- Validate the exact five-minute timing grid, UTC partition ownership,
  fixed-point integer fields, depth arrays and canonical booleans.
- Require one globally unique row for each condition and second index.
- Publish one create-new, content-addressed CSV with a checksum-bound manifest.

## Exclusions

- Mutation of the sealed partitioned source, interpolation, deduplication by
  preference, value repair, feature engineering, labels, model training,
  strategy authority, paper execution or live execution.

## Acceptance criteria

- The merged row count equals the source root and partition manifest totals.
- Every accepted condition has exactly the indices `0..299`.
- Altered checksums, schema drift, duplicate keys, incomplete conditions,
  malformed fixed-point values, timing inconsistency and path escape fail
  before publication.
- Repeated identical source evidence reuses the same verified revision.
- The merged CSV and manifest are independently checksum-verifiable and remain
  marked as derived, non-financial research evidence.
