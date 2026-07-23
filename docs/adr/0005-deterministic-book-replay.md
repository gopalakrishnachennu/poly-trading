# ADR-0005: Deterministic single-writer order-book replay

- Status: Accepted
- Date: 2026-07-17

## Context

Recorded public events are useful only if the same durable bytes reproduce the
same authoritative state. External decimal strings, snapshots, deltas,
connection epochs, and ordering errors must not be interpreted differently by
live and offline consumers.

## Decision

Decode the versioned public payload into strict fixed-point Rust types and apply
events through one deterministic state machine. The state machine:

- accepts one globally increasing recorder sequence;
- invalidates all books at every epoch start;
- requires a fresh snapshot before applying deltas to a token;
- treats duplicate, decreasing, or gapped sequence as a replay halt;
- rejects crossed snapshots, duplicate levels, malformed decimals, mismatched
  prefix/JSON identity, and events outside an active epoch;
- marks a book non-authoritative after a crossed delta or reported-best
  mismatch and requires a fresh snapshot before readiness;
- represents each side with ordered integer maps;
- produces a stable BLAKE3 digest from explicitly encoded state.

Replay has no network, strategy, wallet, or execution capability.

## Consequences

Historical and future shadow-live consumers can compare state digests. Journal
loss, reordering, or schema drift becomes explicit instead of silently creating
a plausible but incorrect book. Throughput optimization remains secondary to
replay equivalence.
