# ADR 0044: Model Executor Sessions Before Transport

- Status: Accepted
- Date: 2026-07-21

## Decision

Phase 2.28 adds a deterministic offline executor-session protocol before any
credential or transport adapter. A session binds an exact current Phase 2.27
report and plan, a credentialless isolation contract, an exclusive expiring
lease and contiguous exact request templates.

Only recorded simulation observations exist. A simulated acknowledgement is
not an external acknowledgement and cannot claim mutation. Unknown outcomes,
dead-man expiry and restart disable new requests until exact no-mutation
reconciliation. Every request remains bound to one lease, process, session,
template and upstream evidence chain.

## Consequences

- Protocol recovery and ambiguity handling can be certified without secrets.
- No state emitted by this phase authorizes or proves a deployment.
- External transport remains a separately reviewed future boundary.
- Lease loss and uncertainty fail closed rather than triggering retries.
