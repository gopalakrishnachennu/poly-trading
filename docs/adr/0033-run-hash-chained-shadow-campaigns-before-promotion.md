# ADR 0033: Run Hash-Chained Shadow Campaigns Before Promotion

## Status

Accepted for Phase 2.17.

## Decision

Introduce `shadow-session-campaign` as a bounded single writer above one Phase
2.16 gateway. A sealed manifest commits to exact recorded-session identities,
windows, recording digests, required fault scenarios, step count and the final
hash-chain digest before execution begins.

Runtime replay is accepted only inside its exact active session. Global control
steps may renew certification, update synthetic heartbeat state, apply recorded
fixtures, evaluate expiry or provide digest-bound recovery evidence. Scenario
coverage is derived from accepted Phase 2.16 commands and outcomes rather than
reported by the caller.

Finalization always emits an evidence bundle. Eligibility requires the complete
schedule and session set, all required coverage, a ready non-halted gateway,
zero reserved cash and zero pending conversions. The bundle is versioned,
canonical, checksummed and internally digest-bound. Eligibility requires a
separate operator decision and grants no promotion or deployment authority.

## Consequences

Multi-day safety campaigns become reproducible and independently auditable.
Missing coverage or unresolved state remains explicit. The runner introduces
no credential, signer, authenticated transport, RPC, wallet, relayer, automatic
retry, deployment, promotion or live order/transaction capability.
