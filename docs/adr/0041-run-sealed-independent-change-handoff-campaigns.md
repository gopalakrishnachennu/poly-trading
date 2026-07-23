# ADR 0041: Run Sealed Independent Change-Handoff Campaigns

## Status

Accepted for Phase 2.25.

## Decision

Introduce an offline `deployment-change-campaign` owner above Phase 2.24. One
immutable manifest commits to independent plan cases, exact child command
schedules, expected terminal classes, restart boundaries, required coverage and
the final result-chain digest before execution.

Every case executes through a fresh authentic Phase 2.24 owner. Coverage is
derived from accepted child outcomes. Approval renewal requires distinct plans
with fresh dual-control sets; approval expiry requires the child authority to
reject a late permission. Restart drills rebuild a fresh owner from the exact
accepted prefix and compare complete state before continuing.

Final evidence is canonical and non-authorizing. Eligibility still requires a
future manual decision and cannot trigger infrastructure activity.

## Consequences

Change-handoff operational drills become reproducible without creating a
deployment gateway. Phase 2.25 adds no credential, signature, authenticated
transport, cloud SDK, Kubernetes client, executable request, deployment,
traffic shift, rollback execution, wallet, RPC or live-trading capability.
