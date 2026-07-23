# Institutional Delivery Roadmap

## Objective

Complete the platform from the Phase 2.32 offline safety baseline through a
controlled-production release candidate without weakening any financial,
recovery, reconciliation, or authority boundary.

This roadmap is one continuous delivery program. A phase advances only after
its specification, implementation, failure tests, recovery evidence,
documentation, and applicable formal model pass. Later-phase code cannot turn
an earlier missing gate into an implicit success.

## Completion meanings

- **Code complete** means deterministic implementation and local certification
  pass without external authority.
- **Environment certified** means the named real infrastructure has produced
  fresh evidence under the same sealed configuration.
- **Live eligible** means all prerequisite evidence is current and explicit
  human authorization exists. It does not guarantee a trade or profit.
- **Live complete** requires real canary evidence. It cannot be simulated,
  inferred, or marked complete without credentials, eligible infrastructure,
  capital authorization, and operator approval supplied outside the repository.

## Remaining phases

### Phase 2.33 — Credential-provider adapter certification

Certify recorded opaque-handle acquisition, attestation, rotation, revocation,
quota/outage behavior, split-brain prevention, and disaster recovery. No key
material, provider credentials, cryptographic signing, network connection, or
external mutation is permitted. Completion closes Phase 2.

### Phase 3.0 — Durable infrastructure adapters

Introduce ports and deterministic adapters for PostgreSQL ledger persistence,
Redpanda event streaming, ClickHouse analytics, and Parquet object archives.
Prove idempotency, ordering, backpressure, corruption handling, migration,
restore, and loss/replay boundaries before environment certification.

### Phase 3.1 — Production security boundary

Introduce an isolated signer protocol, Vault/KMS/HSM provider boundaries,
workload identity, least privilege, rotation/revocation, audit evidence, rate
ceilings, and two-person activation. Strategies remain unable to sign or
submit. Local certification uses fake providers and contains no secret values.

### Phase 3.2 — Live read-only venue integration

Compose public and authenticated read-only venue channels, market parameters,
exchange modes, heartbeat, restart, rate-limit and reconnect handling. Order
submission and cancellation remain disabled. Environment completion requires
eligible-network evidence from the target region.

### Phase 3.3 — Blockchain and wallet observation

Add read-only multi-provider RPC observation for collateral, allowances, CTF
inventory, transaction status, finality and reorg handling. Provider
disagreement, stale heads, chain mismatch, and provenance loss halt readiness.

### Phase 3.4 — Continuous shadow-operation certification

Run the complete read-only system continuously with bounded resources,
checkpoint recovery, failover drills, multi-hour rollover, feed partitions,
dead-man faults and signed operator evidence. Code completion includes a
deterministic accelerated campaign; environment certification requires a real
multi-day soak.

### Phase 3.5 — Live-data paper-trading composition

Drive strategy proposals, paired risk, capital staging, paper execution,
settlement simulation and accounting from captured live data. Require
event-time/receive-time correctness, conservative queue models, latency,
partial fills, unknown states and walk-forward evidence. No external mutation.

### Phase 3.6 — Authenticated no-submit certification

Activate isolated production identity and authenticated observation while the
submission capability remains independently absent. Prove session rotation,
revocation, provider outage, disaster recovery, unknown-state reconciliation
and the physical/logical no-submit boundary.

### Phase 3.7 — Micro-capital canary controls

Implement the live canary controller for complete-set opportunities only:
fixed tiny capital, exact market allowlist, dual control, capital floor,
session loss ceiling, exposure caps, kill switch, dead-man cancellation and
rollback. Code completion grants no capital or execution authority. Live
completion requires explicit operator authorization and real canary evidence.

### Phase 3.8 — Controlled-production release

Implement staged capital ceilings, multi-region health, release governance,
incident response, disaster recovery, rollback, continuous reconciliation and
evidence expiry. Production eligibility is revocable and always permits
`NO_TRADE`. No hourly return or principal guarantee is represented.

### Phase 4.0 — Read-only operator terminal projection

Connect validated current public BTC/ETH hourly identity, complementary books
and exact hour-open reference data to a versioned local operator projection.
Require atomic asset publication, strict fixed point, bounded responses,
independent source/receive/projection time, client freshness, and fail-closed
rollover. No accounting, wallet, authenticated transport, risk or order
authority is introduced.

## Gate order

```text
2.33 offline certification
  -> 3.0 durable data plane
  -> 3.1 security boundary
  -> 3.2 live read-only venue
  -> 3.3 read-only chain truth
  -> 3.4 shadow soak
  -> 3.5 live-data paper execution
  -> 3.6 authenticated no-submit
  -> 3.7 micro-capital canary
  -> 3.8 controlled production
  -> 4.0 read-only operator terminal projection
```

No phase may bypass an earlier gate. Missing external evidence produces an
attributable blocked or ineligible result, never fabricated completion.

## Code completion state

Phases 2.33 through 4.0 are implemented at their defined local code gates. Phase 3.8 emits
only revocable code eligibility. Live canary and controlled-production
completion remain external operational gates and cannot be inferred from this
roadmap, tests or reports.
