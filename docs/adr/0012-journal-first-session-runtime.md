# ADR 0012: Journal Session Commands Before Coordination

## Status

Accepted

## Context

Phase 1.8 could deterministically replay in-memory coordination frames, but a
process failure between receiving a frame and applying it had no dedicated
durable session boundary. A runtime also needs bounded ownership and a strict
way to reject checkpoints that disagree with authoritative event history.

## Decision

Introduce `session-runtime` as a read-only, single-writer Tokio boundary. Every
immutable identity registration and complete coordination frame is encoded in
a versioned bounded schema, appended to the existing segmented journal, and
device-synced before coordinator application. Append or sync failure cannot
mutate coordinator state and poisons the live instance.

Checkpoints attest only a sequence and coordinator digest. Recovery replays the
journal and verifies the digest at that exact prefix before processing later
records. Checkpoints are not independent state authority. This initially favors
simple, strong recovery over startup speed.

## Consequences

- Durable replay and live application use identical coordinator transitions.
- A state-invalid command is retained for diagnosis and causes terminal halt;
  an operator must repair or explicitly recover the journal.
- Ingress is bounded and reports full/closed rather than silently dropping.
- No external feed, strategy, credential, signing, or order capability enters
  the runtime.
