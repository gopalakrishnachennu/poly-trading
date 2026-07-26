# Specification 088: Consolidated Paper Trade History

## Objective

Publish one strategy-attributed CSV containing every recorded paper trade while
keeping verified core paper fills, retired legacy simulations, and real-money
activity unambiguously separate.

## Scope

- Validate every core paper-campaign journal record against its BLAKE3 digest,
  campaign identifier, and contiguous sequence.
- Join every core `filled_pair` event to its exact `PAIR_BUY` decision by event
  time, asset, and condition identifier.
- Include retired directional and intraday simulator records as historical
  evidence without granting them current paper authority.
- Preserve legacy decimal source values as decimal text; do not reinterpret
  them through binary floating-point arithmetic.
- Attribute the retired intraday strategy from the checksum-bound simulator
  contract because its historical journal omitted the strategy name.
- Publish content-addressed CSV and manifest artifacts with source and output
  checksums.

## Exclusions

- Real-money trades, wallets, credentials, signing, or live order submission.
- Corrections or reinterpretations of historical simulator results.
- Strategy or model promotion.
- Aggregating independent simulator bankrolls into one portfolio result.

## Acceptance criteria

1. Every exported row has a unique trade identifier and a non-empty strategy.
2. Every core row is derived from a verified journal and exact decision/fill
   join.
3. Digest failures, sequence gaps, incomplete journal tails, duplicate trade
   identifiers, or missing required strategy fields fail the export.
4. Every row identifies its simulation mode, authority class, strategy
   provenance, and real-money status.
5. Export counts reconcile exactly with the accepted source records.
