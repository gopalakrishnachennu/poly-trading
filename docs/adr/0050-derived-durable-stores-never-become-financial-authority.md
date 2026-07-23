# ADR 0050: Derived Durable Stores Never Become Financial Authority

- Status: Accepted
- Date: 2026-07-21

## Decision

The PostgreSQL adapter may persist authoritative ledger transactions only when
they retain the exact pre-authorized transaction identity and digest chain.
Redpanda is an ordered distribution mechanism. ClickHouse is a derived
analytics projection. Parquet-compatible object storage is an immutable archive.
None may originate balances, inventory, reservations, risk approvals, signing
permissions or order authority.

Every adapter must prove content idempotency, ordering, explicit backpressure,
corruption halt, schema migration/rollback, restore and replay convergence.
External services are certified separately from deterministic local fixtures.

## Consequences

- Analytical convenience cannot overwrite financial truth.
- A broker acknowledgement is not a ledger commit.
- Archive restore is untrusted until its complete prefix and provenance verify.
- Open-source local fixtures can validate contracts without paid services.
