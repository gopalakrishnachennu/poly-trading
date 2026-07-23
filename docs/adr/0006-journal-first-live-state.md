# ADR-0006: Journal-first bounded live-state delivery

- Status: Accepted
- Date: 2026-07-17

## Context

Live state must use the same envelopes and transition logic as offline replay.
An unbounded queue hides overload, while dropping an event can create a
plausible but incorrect book. Delivering an event before its durable append can
also expose state that recovery cannot reproduce.

## Decision

The public capture boundary appends each envelope to the checksummed journal
before attempting non-blocking delivery to a bounded live-state channel.

- Successful delivery transfers the exact journaled envelope.
- A full or closed channel ends capture immediately and forbids silent retry or
  drop.
- The authoritative live-state actor has one state writer and applies the same
  `ReplayState` transition used offline.
- Transition errors permanently halt that actor instance.
- Readiness requires a synchronized replay epoch and a fresh market event.
- Freshness evaluation receives explicit time in the deterministic core; wall
  clock access is isolated to the Tokio runtime wrapper.
- Live and offline state digests are compared in tests and operational output.
- Recoverable protocol inconsistencies invalidate the affected book and require
  a fresh snapshot; they never preserve `READY` using questionable state.

## Consequences

Backpressure becomes a visible fail-closed incident. The journal remains the
recovery authority if delivery fails. Actor restart must rebuild from durable
events or begin with a new synchronization epoch; it cannot guess missed state.
No broker is required until measured single-process throughput justifies one.
