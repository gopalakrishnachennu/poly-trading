# Phase 5.14 Specification: Official-Only Resolution Backfill

## Objective

Expand independent BTC-hourly sessions whose pinned supplemental catalogue has
an unset resolution, while trusting only a closed official Polymarket market for
the final binary label.

## Scope

- Continue to require exact official market ID, condition, token mapping,
  opening, closing and rules provenance.
- Accept supplemental resolution `-1` only as an absent label.
- Require the official market to be closed with exact binary outcome prices.
- Preserve and compare supplemental `0`/`1` labels when present.
- Record counts of supplemental-confirmed and official-only labels.

## Exclusions

- Inferring labels from token prices, titles, BTC candles or archive fields;
  accepting unresolved/disputed official markets; model promotion or orders.

## Acceptance criteria

- An official/supplemental identity mismatch always rejects.
- A present supplemental label that conflicts with official evidence rejects.
- A missing supplemental label can publish only after official binary closure.
- Official responses and all derived artifacts remain create-new and
  checksum-bound.
