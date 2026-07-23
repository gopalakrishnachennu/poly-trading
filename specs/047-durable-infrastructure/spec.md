# Phase 3.0 Specification: Durable Infrastructure Adapters

## Objective

Define and certify the deterministic durable-data boundary for PostgreSQL,
Redpanda, ClickHouse and Parquet-compatible object archives while preserving
PostgreSQL/local-ledger authority, exact ordering, idempotency, corruption
halts, bounded backpressure, reversible migration and replay convergence.

## Authority model

- PostgreSQL is an authoritative durable projection only when it exactly
  preserves an already-authorized ledger transaction and its digest chain.
- Redpanda distributes ordered immutable events but cannot originate financial
  facts or authorize exposure.
- ClickHouse is derived analytics and cannot feed authoritative balances,
  positions, reservations, risk or execution decisions.
- Parquet/object archives are immutable replay/audit sources. Restore requires
  checksum, manifest, sequence and provenance verification before use.

## Scope

- Vendor-specific, credentialless contracts behind deterministic Rust ports.
- Exact backend, cluster, region, schema, stream/table/bucket and TLS bindings.
- Fixed scenario matrix for commit, idempotent replay, idempotency conflict,
  sequence gap, backpressure, corruption, forward/rollback migration, restore
  and replay convergence on all four backend classes.
- Conservative dispositions with no automatic retry or authority promotion.
- Monotonic schema epochs and digest-chained record identities.
- Journal-first certification, checkpoints and create-new evidence reports.
- Local open-source fixtures; external environment certification remains a
  distinct evidence gate.

## Exclusions

- Database, broker, analytics or object-store credentials
- Network clients, sockets, DNS, TLS sessions or external mutations
- Managed-service requirement or paid infrastructure
- Live financial migration, production deployment or order submission
- Treating derived stores as ledger, reconciliation or risk authority

## Acceptance criteria

- Missing, stale, substituted, incomplete or authority-bearing Phase 2.33
  evidence fails registration.
- Exactly one contract exists for each required backend, with unique identities
  and correct immutable authority class.
- Records are nonzero, sequence-contiguous, previous-digest-bound and preserve
  event time separately from receive time.
- Duplicate idempotency identity with identical content is a no-op; conflicting
  reuse and sequence gaps halt.
- Backpressure produces bounded explicit backoff without dropping or silently
  acknowledging a record. Corruption halts and cannot be skipped.
- Migration epochs are contiguous. Rollback binds the exact forward migration
  and restores the prior schema digest.
- Restore and replay convergence require exact manifest, prefix and terminal
  state digests; analytical or archive evidence cannot create financial truth.
- Completion requires the full backend/scenario Cartesian matrix and grants no
  credential, provider, deployment, trading, capital or submission authority.
- Tests and TLA+ cover ordering, idempotency, backpressure, corruption,
  migrations, restore, convergence, authority separation and absorbing halt.
