# ADR 0090: Single-file merge remains derived evidence

- Status: Accepted
- Date: 2026-07-26

## Decision

The human-readable single-file BTC five-minute dataset is published as a new
content-addressed derivative. The sealed hourly partitions remain unchanged and
authoritative for export provenance. The merger verifies every source checksum
and all per-market grid invariants before writing one header followed by every
validated source row in canonical UTC-partition order.

## Consequences

Researchers can inspect or import one CSV without manually combining 34 files.
The merged file does not improve capture density: a grid row may still carry
the most recent authentic observation, with age and freshness fields making
that explicit. It grants no model, strategy, paper, capital or live-trading
authority.
