# Phase 2.24 Specification: Offline Deployment Change Control

## Objective

Consume one exact current Phase 2.23 certificate and deterministically simulate
maintenance-window change control, dual approval, ordered short-lived one-use
manual handoffs, pause/abort and reverse emergency rollback without creating
credentials or contacting a control plane.

## Scope

- Phase 2.23 reports expose exact regions, preflight and rollback-package
  subjects in addition to their existing contract and evidence subjects.
- One immutable plan binds a certified report, ordered maintenance windows,
  contiguous regional change steps, an emergency rollback policy, creation time
  and expiry.
- Release and risk approvals bind the exact plan, remain current and come from
  distinct nonzero opaque operators.
- One change step at a time may receive a short-lived manual permission inside
  an active maintenance window. Each permission is exact-subject and one-use.
- Consuming a permission records only a simulated manual handoff; it does not
  represent deployment, mutation, authentication or success.
- Pause invalidates any outstanding permission and requires explicit resume
  with renewed current dual control and an active window.
- Abort before any handoff is terminal and safe. Abort or a configured severe
  signal after a handoff irreversibly requires rollback.
- Rollback permissions follow consumed steps exactly in reverse order, are
  short-lived and one-use, and record manual handoff only.
- Final reports distinguish simulated completion, safe abort and simulated
  rollback convergence while granting no external authority.

## Exclusions

- Credentials, tokens, certificates, keys, signatures or authenticated RPC
- Cloud/Kubernetes/DNS/load-balancer/artifact-registry clients
- Actual apply, routing, failover, rollback, wallet or trading activity

## Acceptance criteria

- Non-certified, stale, authority-bearing, corrupt or substituted Phase 2.23
  evidence cannot register.
- Windows and steps are bounded, unique, ordered and exact-region bound.
- Approval substitution, rejection, expiry or same-operator dual control cannot
  issue a permission.
- Change permissions are issued only for the next step inside an active window,
  expire at every plan/window/policy boundary and are consumed at most once.
- Pause invalidates an outstanding handoff. No later resume can revive it.
- Post-handoff abort and severe signals cannot complete or clear rollback.
- Rollback convergence is impossible except through reverse consumed-step order.
- Commands are content-idempotent, journal-first and checkpoint-replayable;
  corruption, equivocation and impossible transitions halt absorbingly.
- Tests cover completion, approval/window/expiry gates, permission single-use,
  pause invalidation, abort, severe rollback, reverse ordering, durability and
  corruption.
- A bounded TLA+ model proves ordered handoffs, dual control, rollback order,
  no authority and absorbing halt.
