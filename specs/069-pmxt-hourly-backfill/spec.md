# Phase 5.3 Specification: Selective PMXT BTC/ETH Hourly Backfill

## Objective

Ingest only relevant historical public CLOB events from the PMXT v2 archive
into immutable local BTC/ETH hourly research partitions. External data remains
supplemental evidence: official Polymarket identity is the allowlist authority.

## Scope

- Read explicitly downloaded PMXT v2 Parquet files through a bounded importer.
- Require an immutable official identity manifest with BTC/ETH one-hour
  condition IDs and outcome token IDs.
- Retain only allowlisted tokens, validate event and receive timestamps, event
  type, book JSON, and price/size decimal fields.
- Emit create-new asset/date/hour JSONL partitions with checksummed manifests.

## Exclusions

- Bulk downloading the archive, automatic source discovery, model training,
  paper execution, strategy approval, credentials, wallets, signing and orders.

## Acceptance criteria

- Unknown conditions/tokens are excluded without output; duplicated allowlist
  identity, malformed allowlisted events, invalid timestamps, excessive source
  files, corrupt Parquet and pre-existing output partitions fail before
  publication.
- Output preserves source and receive time, external source identity and all
  CLOB values as strings; no financial values use binary float.
- Output is strictly partitioned by asset / UTC date / UTC hour and includes
  input/output checksums and row counts.
- Tests cover a valid fixture and failure cases.
