# ADR 0027: Compose Paired Policy and Paper Execution Under One Writer

## Status

Accepted for Phase 2.11.

## Decision

The `paired-paper-execution` runtime owns the complete Phase 2.10 policy state
and all paired paper orders. Callers may submit setup and authorization commands
but cannot inject leg lifecycle commands into the policy child. Submission and
accepted simulated exchange observations are the only sources of lifecycle
progression.

Submission consumes one exact, current, unused paired permit and verifies its
stage, leg, candidate, reservation, policy record, and validity. Paper orders
retain source sequence, event and receive time, immutable exchange-order ID,
cumulative fixed-point fill economics, and explicit submitted, delayed,
acknowledged, live, partial, unknown, cancel-pending, fully matched, canceled,
and rejected states.

Each accepted fill validates incremental and cumulative quantity,
consideration, fee, limit price, full-match consistency, fill identity, and
ledger-command uniqueness. It creates exactly one immutable reconciliation
handoff. Handoffs remain pending outputs; this phase does not post accounting or
claim confirmed inventory.

Execution transitions compare the complete staged ledger risk view before and
after policy synchronization. Both reservations must remain identical through
every exposure-bearing and unposted-handoff state. Only an authoritative
zero-fill terminal path can make the Phase 2.10 paired abort safe.

The owner is append-and-sync before mutation, content-idempotent, strictly
replayable, digest-stable, and prefix-checkpointed. Child, provenance, history,
lifecycle, fill, handoff, reservation, or durable failure halts the owner.

## Consequences

Phase 2.11 exercises paired execution and produces settlement handoffs without
creating live venue authority. Settlement confirmation, ledger posting,
split/merge, signing, credentials, wallet access, authenticated transport, and
real order submission remain excluded.
