# Phase 5.6 Specification: Official Verification of Supplemental BTC-Hourly Candidates

## Objective

Convert a bounded third-party BTC one-hour market catalogue into an immutable
official Polymarket identity and resolution manifest before any external price,
trade, or BBO row can enter research.

## Scope

- Read one revision-pinned, size-bounded Parquet market catalogue.
- Select only BTC one-hour candidates in an explicit whole-hour UTC range.
- Retrieve or consume preserved official Gamma market responses for those
  candidate IDs.
- Validate exact market ID, condition ID, `Up`/`Down` token mapping, official
  `eventStartTime`, one-hour close, rules provenance, closed state, and binary
  resolution.
- Emit create-new canonical official evidence, a schema-v2 identity manifest,
  and fixed-label records with checksums and source provenance.

## Exclusions

- Trusting third-party `start_ts`, downloading price/BBO/trade partitions,
  feature generation, model training, paper or live execution, capital,
  credentials, signing, wallets, and orders.

## Acceptance criteria

- Missing, duplicate, non-hourly, unresolved, outcome-mismatched, token-
  mismatched, condition-mismatched, out-of-range, oversized, unsafe, malformed,
  or incomplete candidate coverage rejects the entire publication.
- The official `eventStartTime`, never the third-party creation timestamp,
  defines the session opening.
- Official evidence and labels are immutable, checksum-bound, and preserve
  source provenance. Financial labels contain no binary floating point.
- Tests cover successful validation plus identity, resolution, coverage, and
  timestamp failure.
