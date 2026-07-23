# Phase 2.25 Specification: Offline Change-Handoff Campaign

## Objective

Run multiple immutable Phase 2.24 plans through deterministic recorded command
schedules, exercise maintenance-window, approval, pause, abort, rollback and
restart behavior, and emit checksummed non-authorizing operator evidence.

## Scope

- One campaign manifest binds identity, creation and expiry, exact independent
  plan cases, required scenarios and the final case-chain digest.
- Every case binds one Phase 2.24 policy, a complete immutable command schedule,
  an expected terminal class and an optional deterministic restart boundary.
- A fresh Phase 2.24 owner executes each case. The campaign cannot inject child
  outcomes or coverage claims.
- Restart coverage reconstructs a fresh child from the exact accepted prefix,
  compares the complete child digest and only then continues.
- Multi-window, pause/resume, safe abort and emergency rollback coverage derives
  from authentic child commands and outcomes.
- Approval renewal requires fresh valid dual-control sets on at least two
  independent plans. Approval-expiry coverage requires Phase 2.24 itself to
  reject a permission after an approval boundary.
- Cases run exactly once in manifest order and chain to the prior case result.
- Final evidence separately reports missing cases, scenarios, independence,
  schedule mismatch, child halt mismatch and non-authorizing-state violations.

## Exclusions

- Credentials, keys, signatures, authenticated transport or control-plane RPC
- Cloud, Kubernetes, DNS, load-balancer or artifact-registry clients
- Actual apply, routing, failover, rollback, wallet or trading activity
- Automatic deployment or authority from an eligible evidence result

## Acceptance criteria

- Invalid, duplicate, substituted, overlong or non-independent case subjects
  are rejected before state installation.
- Child commands are bounded, timestamp-monotonic and bind only their case plan.
- Normal cases require an exact digest-valid Phase 2.24 terminal report matching
  the sealed expected status.
- Approval-expiry cases pass only when a permission attempt after the sealed
  approval boundary produces the authentic Phase 2.24 approval halt.
- Restart reconstruction must reproduce the exact complete child digest.
- Eligibility requires every case in order, exact final result-chain digest,
  all required derived scenarios and the configured independent-plan floor.
- Evidence always requires future manual operator action and grants no external
  or live-trading authority.
- Commands are content-idempotent, journal-first and checkpoint-replayable;
  corruption, equivocation and impossible transitions halt absorbingly.
- Tests cover complete multi-window execution, renewal, expiry denial,
  pause/resume, safe abort, emergency rollback, restart reconstruction,
  substitution, incomplete evidence, corruption and sync failure.
- A bounded TLA+ model proves ordered cases, derived coverage gates, restart
  ordering, no authority, one finalization and absorbing halt.
