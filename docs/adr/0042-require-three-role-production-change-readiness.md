# ADR 0042: Require Three-Role Production-Change Readiness

## Status

Accepted for Phase 2.26.

## Decision

Introduce offline `production-change-readiness` governance above canonical
Phase 2.25 evidence. Phase 2.25 evidence is versioned forward to expose exact
certificate, preflight, plan and rollback subject sets.

One sealed candidate binds those subjects to immutable release, binary,
configuration, infrastructure and observability digests, a regression baseline
and governance policy. Only fresh independently identified eligible campaigns
contribute. Diversity gates are separate and regression floors round upward.

Release, risk and operations decisions must approve the exact unchanged
candidate, remain current and use three distinct opaque operator labels. These
labels provide accountability only; they are not credentials or signatures.
Final readiness remains evidence and has no executable authority.

## Consequences

Production-change readiness becomes reproducible, regression-aware and
non-substitutable without adding a deployment gateway. Phase 2.26 adds no
credential, signer, authenticated transport, cloud SDK, Kubernetes client,
deployment, traffic shift, rollback execution, wallet, RPC or live trading.
