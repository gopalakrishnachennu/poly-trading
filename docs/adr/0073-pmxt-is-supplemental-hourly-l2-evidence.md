# ADR 0073: Use PMXT only as provenance-bound supplemental L2 evidence

- Status: Accepted
- Date: 2026-07-25

## Decision

PMXT v2 CLOB archives may enter research only through an explicit official
BTC/ETH hourly identity manifest. The importer retains known condition/token
pairs and validates timestamps and payload fields before publishing an
immutable, checksummed local partition. PMXT is not an execution, settlement,
or market-identity authority.

## Consequences

The platform can backfill historic public CLOB state without accepting an
unbounded third-party market universe. Malformed, unknown or conflicting data
does not enter training. The importer creates no model, paper, wallet,
credential, signing or order authority.
