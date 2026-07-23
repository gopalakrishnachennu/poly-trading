# ADR 0035: Simulate Canary Control Before Any Deployment Boundary

## Status

Accepted for Phase 2.19.

## Decision

Introduce `canary-rollout-simulator` as a bounded offline single writer above
one exact Phase 2.18 canary-eligibility record. A sealed plan binds the record,
matching rollback criteria, maintenance windows, increasing target basis points,
and stage observation limits before lifecycle processing begins.

Start, advance and resume require an unexpired record, an active maintenance
window and a current health frame proving every independent stack component,
reconciliation and capital floor are healthy. Ordinary health loss pauses.
Capital-floor breach, threshold violations and timeouts latch a simulated
rollback requirement that later healthy evidence cannot erase.

Restart preserves the exact stage and rollback latch. Recovery requires a new
epoch and post-restart health, returns paused, and cannot resume automatically.
Operator identifiers remain unauthenticated accountability data. Final reports
are evidence only and grant no deployment or rollback-execution authority.

## Consequences

Rollout ordering, time windows, degraded states and abort decisions become
replayable before any external control plane exists. The component cannot route
traffic, allocate capital, deploy, roll back, authenticate, sign, access an RPC
or wallet, or submit any live order or transaction.
