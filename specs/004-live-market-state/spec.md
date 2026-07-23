# Specification 004: Bounded Live Market State

## Purpose

Apply journaled public events to authoritative in-process state with explicit
backpressure, feed freshness, and offline replay equivalence.

## Requirements

### LMS-001 Journal-first delivery

Every envelope is appended before live delivery. The delivered envelope must be
byte-equivalent to the journaled envelope.

### LMS-002 Bounded ingress

Ingress uses a configured positive-capacity bounded channel. Full and closed
channels are hard capture errors; events are never dropped, overwritten, or
queued without a bound.

### LMS-003 Single writer

Only one actor task mutates authoritative `ReplayState`. Readers receive
immutable health snapshots and digests.

### LMS-004 Fail-closed transitions

Sequence, epoch, payload, or book errors move the actor to `HALTED`. A halted
actor cannot become ready or accept further authoritative transitions.

### LMS-005 Readiness and freshness

`READY` requires a synchronized epoch and a market event whose receive timestamp
is within the configured staleness budget. Starting, collecting snapshots,
stale, shutdown, closed, and halted states are not ready.

### LMS-006 Deterministic health core

Freshness is evaluated using an explicit `now_ns`. Clock regression is an error.
The runtime wrapper may read the wall clock only to create health-tick inputs.

### LMS-007 Replay equivalence

For the same ordered envelopes, the live actor and offline replay produce the
same state digest and terminal sequence.

### LMS-008 Read-only scope

This phase contains no authenticated channel, wallet, signer, strategy, order,
cancellation, or position capability.

## Acceptance criteria

- Journal-first delivery tests prove full/closed channels return errors after
  the event is recoverable from the journal.
- Actor tests cover starting, snapshot collection, ready, stale, shutdown,
  closed, and halted states.
- A transition failure leaves the last valid digest visible and permanently
  disables readiness.
- Live and offline replay of the same fixture have identical digest/sequence.
- A read-only executable captures through the bounded actor pipeline.
- Formatting, Clippy, all Rust tests, and TLC pass.
