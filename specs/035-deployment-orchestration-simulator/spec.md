# Phase 2.22 Specification: Offline Deployment and Rollback Orchestration

## Objective

Consume one exact current Phase 2.21 preflight report and deterministically
simulate ordered regional deployment waves, independent health gates, explicit
pause/abort, irreversible rollback, reverse-order convergence and restart
recovery without contacting a cloud or cluster control plane.

## Scope

- Phase 2.21 reports expose exact regions, rollback-package digest and package
  expiry in addition to their unchanged package and fleet subjects.
- One single writer owns one immutable orchestration plan.
- Waves are ordered, nonempty and collectively contain every preflight region
  exactly once. Each wave has bounded observation and maximum duration.
- Health frames are contiguous, fresh, digest-bound and independently cover
  package, service, risk, reconciliation and capital-floor state per region.
- Start, wave advance and resume require fresh healthy evidence for the exact
  active/next regional scope.
- Ordinary service/risk degradation pauses. Reconciliation or capital-floor
  failure and wave/plan timeout irreversibly latch rollback.
- Operator abort before activation terminates safely. Abort after activation
  requires rollback rather than declaring completion.
- Rollback observations bind the exact rollback package and must converge every
  activated region exactly once in reverse activation order.
- Restart preserves wave, activated-region, rollback and convergence state.
  Recovery requires a new epoch and evidence and never resumes automatically.
- Final reports are canonical, checksummed and grant no deployment, rollback,
  credential, cloud-control or trading authority.

## Exclusions

- Cloud, Kubernetes, DNS, load balancer, routing or artifact-registry clients
- Credentials, keys, signatures, authenticated transport, RPC or wallet access
- Actual deployment, traffic shift, rollback, order or transaction submission

## Acceptance criteria

- Non-ready, expired, authority-bearing, corrupt or substituted Phase 2.21
  evidence cannot register.
- Missing, duplicate, extra or reordered regional wave coverage cannot register.
- No wave starts or advances without current exact-scope health.
- Ordinary degradation pauses without automatic resume; severe failure and
  timeouts latch rollback and cannot be cleared.
- Rollback completion is impossible until every activated region converges in
  exact reverse order against the bound rollback package.
- Restart recovery preserves progress and requires explicit operator resume or
  continued rollback.
- Commands are content-idempotent, journal-first and checkpoint-replayable;
  corruption, equivocation and impossible transitions halt absorbingly.
- Tests cover completion, health gating, pause/resume, severe rollback, abort,
  reverse convergence, restart, expiry, substitution, durability and corruption.
- A bounded TLA+ model proves wave ordering, health gates, rollback convergence,
  recovery ordering, no authority and absorbing halt.
