# ADR 0058: Production eligibility is revocable and evidence-bound

## Status

Accepted

## Context

A locally correct release controller cannot prove legal eligibility, allocate
capital, activate credentials or establish that a real target environment is
healthy. Production conditions also decay: regional observations, reconciliation
and operator approvals can become stale after a report is generated.

## Decision

Phase 3.8 emits only a code-eligibility report. It binds an exact Phase 3.7
report to immutable artifact and operational subjects, monotonic signed
fixed-point ceilings, three distinct opaque operator labels, independently
current regional health and a complete adverse-case matrix. Evidence expires,
finalization rechecks freshness, revocation is irreversible, and `NO_TRADE`
remains permitted at all stages.

The controller contains no capital, credential, signer, deployment client,
authenticated transport or order-submission capability. Live production
completion requires external authorization and real evidence outside this code
certificate.

## Consequences

- Code completion cannot be mistaken for live-production completion.
- A stale or revoked subject cannot retain eligibility.
- Increasing capital requires explicit ordered stages and current evidence.
- Operational safety never depends on generating a trade or return.
