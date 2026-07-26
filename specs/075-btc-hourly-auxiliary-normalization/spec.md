# Phase 5.9 Specification: BTC-Hourly Spot and Trade-Flow Normalization

## Objective

Add causal Binance BTC/USDT settlement-source and Polymarket trade-flow evidence
to the verified BTC-hourly research dataset without trusting floating-point
third-party values or unverified market identities.

## Scope

- Require revision/checksum-pinned spot and BTC-hourly trade Parquet sources.
- Keep only Binance `btcusdt` spot values and officially verified market/token
  trade rows.
- Validate source bounds, schemas, timestamps, token/outcome mapping, sides,
  prices and sizes.
- Deduplicate on-chain fills by transaction hash/log index.
- Convert prices and sizes once into fixed-point integer micros.
- Publish compressed normalized Parquet files and a checksum manifest.

## Exclusions

- Using Chainlink as the settlement truth for these Binance-resolved markets,
  trusting user profiles, model training, strategy promotion, paper/live
  execution, capital, credentials, wallets, signatures, or orders.

## Acceptance criteria

- Checksum, schema, identity, token, outcome, timestamp, side, numeric, size or
  row-bound failures reject publication.
- Normalized financial fields are signed 64-bit fixed-point integers.
- Spot and trade rows are chronological and duplicate on-chain fills cannot
  inflate flow.
- Output is create-new, checksum-bound and explicitly supplemental.
