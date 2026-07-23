# ADR 0040: Use Expiring One-Use Manual Change Handoffs

## Status

Accepted for Phase 2.24.

## Decision

Introduce an offline `deployment-change-control` owner above one exact current
Phase 2.23 certificate. Two distinct opaque release and risk operators must
approve the unchanged plan. The owner may issue only the next ordered step as a
short-lived, one-use manual handoff inside a sealed maintenance window.

Pause invalidates outstanding handoffs. Abort before any consumed handoff is
safe; afterward, abort and severe signals irreversibly require rollback.
Rollback handoffs cover consumed steps in reverse order and remain simulated,
short-lived and one-use.

## Consequences

Deployment authorization sequencing and emergency rollback policy become
replayable without representing a real permission. Phase 2.24 adds no token,
credential, signature, executable request, authenticated transport, cloud SDK,
Kubernetes client, deployment, routing, rollback execution, wallet, RPC or live
trading path.
