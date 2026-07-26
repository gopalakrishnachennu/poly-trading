# Phase 5.7 Specification: Conservative Supplemental BTC-Hourly BBO Normalization

## Objective

Normalize a revision- and checksum-pinned BTC one-hour BBO Parquet source into
fixed-point, replay-compatible research evidence using only officially verified
market identities.

## Scope

- Require the official identity manifest and resolution labels from Phase 5.6.
- Validate source size, checksum, schema, row count, market/token/outcome
  identity, timestamp ordering, price bounds, spread, and displayed size.
- Preserve source and local receive timestamps.
- Convert external numeric values once at the ingestion boundary into integer
  micros using conservative ask/bid rounding.
- Emit a bounded one-second derived BBO tape plus a checksum manifest. The
  original compressed Parquet remains the raw supplemental record.

## Exclusions

- Treating BBO as full L2 depth, passive queue-fill inference, model training,
  strategy approval, paper/live execution, credentials, wallets, or orders.

## Acceptance criteria

- Unknown, mismatched, crossed, invalid, regressing, oversized, excessive, or
  checksum-mismatched input rejects publication.
- Only rows inside the official session interval enter the derived tape.
- At least the research policy's minimum number of independently verified
  sessions must have usable BBO evidence.
- Output contains integer fixed-point levels and immutable source/output
  checksums and is explicitly non-authoritative.
