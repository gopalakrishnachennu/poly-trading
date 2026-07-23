# Phase 2.19 Specification: Offline Canary Rollout and Abort Simulation

## Objective

Consume one exact unexpired Phase 2.18 canary-eligibility record and simulate a
bounded rollout through immutable maintenance windows and observation stages,
with independent health gates, deterministic pause/abort/rollback behavior,
restart recovery, and a checksummed non-executing report.

## Scope

- One single writer owns one sealed rollout plan and lifecycle.
- The plan embeds the exact Phase 2.18 record, its matching rollback criteria,
  ordered non-overlapping maintenance windows, and strictly increasing bounded
  rollout stages.
- Health frames are digest-bound, contiguous, timestamped, expiring, and expose
  strategy, risk, market-feed, user-feed, reconciliation, capital-floor,
  unknown-state, session-loss, and consecutive-fault state independently.
- Starting, advancing, and resuming require a current completely healthy frame,
  an active maintenance window, an unexpired candidate, and no rollback latch.
- Ordinary stack-health loss pauses the simulation. Capital-floor breach,
  excessive unresolved/unknown time, excessive loss, fault threshold, stage
  timeout, or total-plan timeout latches simulated rollback requirement.
- Operator pause and abort use opaque accountability identifiers only. They are
  not credentials, signatures, authentication, or execution authority.
- Restart preserves stage and rollback state. Recovery requires a new epoch,
  nonzero evidence digest, and a post-restart healthy frame; it returns paused
  and still requires explicit resume.
- A terminal report binds the complete plan and transition history and grants no
  deployment, rollback execution, credential, signing, wallet, RPC, order,
  transaction, or live authority.

## Exclusions

- Real deployment, process control, traffic routing, capital allocation
- Credentials, private keys, signatures, identity-provider integration
- Authenticated transport, RPC, wallet, relayer, exchange, or cloud clients
- Automatic rollback execution, order submission, or production capital

## Acceptance criteria

- Non-eligible, expired, authority-bearing, corrupt, or substituted Phase 2.18
  records fail closed before plan installation.
- Rollback criteria digest, rollout windows, stage ordering, basis points,
  observation durations, total duration, and plan digest are exact and bounded.
- Start/advance/resume cannot bypass health freshness, maintenance windows,
  record expiry, minimum observation time, stage order, pause, restart, abort,
  rollback, or terminal state.
- Health sequence/time/digest equivocation halts; ordinary unhealthy state pauses;
  every configured severe threshold latches rollback and cannot be cleared.
- Operator abort is terminal and cannot be represented as successful completion.
- Restart recovery requires post-restart health and never resumes automatically.
- Exact duplicate commands are idempotent; conflicting identities or lifecycle
  regression halt transactionally.
- Journal replay and checkpoint recovery reproduce the complete lifecycle digest.
  Reports are create-new, bounded, canonical, checksummed, and digest-verified.
- Tests cover success, maintenance-window denial, stale/unhealthy health, every
  rollback class, threshold boundaries, pause/resume, abort, restart recovery,
  substitution, durability failure, corruption, and deterministic replay.
- A bounded TLA+ model proves ordered stages, health/window gates, rollback
  absorption, restart-before-recovery ordering, terminal exclusivity, no
  execution authority, and absorbing integrity halt.
