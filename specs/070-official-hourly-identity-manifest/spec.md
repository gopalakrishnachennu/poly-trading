# Phase 5.4 Specification: Official BTC/ETH Hourly Identity Manifest

## Objective

Create a checksummed, immutable BTC/ETH hourly identity allowlist from official
Polymarket Gamma event metadata before any supplemental archive is imported.

## Scope

- Accept bounded, immutable downloaded Gamma pages or explicitly requested,
  bounded read-only pages from the pinned official Gamma endpoint.
- Require a complete requested range: exactly one validated BTC and ETH
  `Up`/`Down` market per UTC hour.
- Validate exact UTC hour boundaries, condition IDs, outcome/token ordering and
  rule provenance, then write a create-new canonical manifest.

## Exclusions

- Market selection from a third-party archive, model training, paper execution,
  capital, credentials, wallets, signing, order placement and order submission.

## Acceptance criteria

- Missing, duplicated, ambiguous, malformed, oversized, unsafe, out-of-range,
  or non-hourly identities reject the requested manifest.
- Each manifest preserves input source digests and a rules fingerprint. The
  existing PMXT importer can consume its `assets` allowlist without conversion.
- Tests cover a complete two-asset hour, missing coverage, duplicate candidate,
  invalid tokens, and malformed timestamps.
