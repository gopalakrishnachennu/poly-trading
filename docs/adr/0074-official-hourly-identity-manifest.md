# ADR 0074: Derive hourly identity allowlists only from official Gamma metadata

- Status: Accepted
- Date: 2026-07-25

## Decision

BTC/ETH one-hour condition and token identities used by supplemental research
imports are constructed from pinned official Polymarket Gamma event metadata.
The builder requires full requested-hour coverage, exact hour boundaries and
exact `["Up", "Down"]` token ordering. It fingerprints the rules provenance and
records source digests. It never infers an opening time from a closing time.

## Consequences

An archive cannot silently decide which market is BTC or ETH. Incomplete or
ambiguous history produces no allowlist, so downstream import stays halted.
This is a read-only research boundary and grants no model, paper, wallet,
credential, signing or execution authority.
