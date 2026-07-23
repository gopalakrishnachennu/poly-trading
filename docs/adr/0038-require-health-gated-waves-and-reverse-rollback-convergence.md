# ADR 0038: Require Health-Gated Waves and Reverse Rollback Convergence

## Status

Accepted for Phase 2.22.

## Decision

Introduce a bounded offline `deployment-orchestration-simulator` above one exact
Phase 2.21 report. Every region appears in exactly one ordered wave. Start,
advance and resume require current region-specific health. Ordinary degradation
pauses; reconciliation, capital-floor or timeout failure irreversibly requires
rollback.

Rollback must cover every activated region exactly once in reverse activation
order using the report-bound rollback package. Restart retains all progress and
requires explicit recovery; it never resumes deployment automatically.

Reports are evidence only and all external authority flags remain false.

## Consequences

Wave ordering, partial-fleet incidents and rollback convergence become
replayable before any control-plane integration exists. This phase adds no
credential, network client, cloud/Kubernetes adapter, deployment, routing,
rollback execution, wallet, RPC or live-trading path.
