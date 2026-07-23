# Phase 3.4 Specification: Continuous Shadow-Operation Certification

## Objective

Certify a deterministic accelerated continuous-operation campaign over the
complete read-only venue and chain observation boundary, including resource
budgets, hourly rollover, partitions, dead-man behavior, checkpoint restart and
failover recovery without claiming a real multi-day environment soak.

## Scope

- Exact current Phase 3.3 local report and immutable runtime/config bindings.
- Contiguous campaign ticks preserving event, receive and monotonic time.
- Bounded queue, memory, file, journal and latency observations.
- Explicit hour/session rollover and checkpoint restart recovery.
- Venue-feed, chain-provider and dead-man disruption/recovery drills.
- Isolated clock-regression and durable-corruption halt fixtures.
- Distinct opaque operator accountability labels on final evidence.
- Journal replay, prefix checkpoints and create-new evidence bundles.

## Exclusions

- Real multi-day or target-region certification
- Credentials, signatures, RPC/venue connection, wallet or mutation
- Deployment, trading, capital or submission authority
- Treating accelerated time as elapsed real time

## Acceptance criteria

- Registration rejects stale, incomplete, live-certified, substituted or
  authority-bearing Phase 3.3 evidence.
- Tick sequence is contiguous and event/receive/monotonic chronology cannot
  regress. Accelerated and real elapsed durations remain separate.
- Every resource dimension is bounded independently. Excess denies progress and
  cannot be averaged away by another healthy dimension.
- Rollover is contiguous and checkpoint restart clears readiness until exact
  digest-bound recovery completes.
- Venue partition, chain partition and dead-man drills clear readiness before
  recovery; recovery requires explicit no-mutation evidence.
- Clock regression and durable corruption are isolated halt-class fixtures and
  never contribute operational state.
- Finalization requires all scenarios, minimum accelerated duration, minimum
  rollover count, no recovery debt, current health and two distinct operators.
- Report status is locally certified, real multi-day certification is false and
  every credential, connection, mutation, deployment, trading and submission
  authority flag is false.
- Tests and TLA+ cover continuity, budgets, invalidation, recovery, evidence
  completeness, no mutation and absorbing halt.
