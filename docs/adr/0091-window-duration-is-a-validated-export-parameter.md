# ADR 0091: Window duration is a validated export parameter

- Status: Accepted
- Date: 2026-07-26

## Decision

One deterministic exporter and merger implementation owns BTC `5m`, `15m` and
`1h` per-second research datasets. Duration is not an arbitrary operator value:
only the exact pairs `(300000 ms, 300 rows)`, `(900000 ms, 900 rows)` and
`(3600000 ms, 3600 rows)` are accepted. Market identity must use the same exact
duration and UTC alignment.

## Consequences

All timeframes share identical integrity and quality semantics without copied
logic drifting independently. A complete grid remains a sampling surface, not
a claim that the REST collector received a new book every second. Timeframes
remain separate source revisions and are never synthesized from each other.
