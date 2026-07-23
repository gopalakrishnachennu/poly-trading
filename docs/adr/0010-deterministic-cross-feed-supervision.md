# ADR 0010: Deterministic cross-feed supervision

- Status: Accepted
- Date: 2026-07-17

## Context

An authoritative Polymarket book and a healthy Binance reference feed can fail
independently. Neither feed's local `READY` flag is sufficient for a future
decision. A unified gate must also reject stale observations, excessive
cross-feed arrival skew, bad source time, local clock regression, and snapshot
history equivocation.

## Decision

Introduce a pure `feed-supervisor` crate. It accepts immutable market and
reference snapshots plus explicit `now_ns`; it owns no connection and never
reads the system clock.

`READY` requires:

```text
Polymarket actor READY
Binance reference replay READY
all required receive timestamps present and within source-specific budgets
all known source event times within lag/future-skew budgets
market/reference latest receive skew within its budget
non-regressing epochs and sequences
stable digest when sequence does not advance
```

Budget comparisons are inclusive: an age or skew exactly equal to its limit is
accepted. Missing required timestamps fail closed.

Recoverable non-ready modes are separate: market unavailable, reference
unavailable, market stale, reference stale, cross-feed skew, source-event lag,
and source-event future skew.

The following permanently halt the supervisor:

- local evaluation-clock regression;
- a receive timestamp later than evaluation time;
- market or reference epoch/sequence regression;
- a feed digest changing without sequence advancement.

State updates are transactional. An integrity failure retains the last accepted
feed markers, records a halt reason, and can never return to `READY`. The stable
digest is derived from explicit fields rather than Rust memory layout.

## Consequences

- Future strategy code consumes one non-bypassable cross-feed readiness result.
- A feed can recover from ordinary staleness without restarting the supervisor.
- Clock/history integrity failures require operator-controlled recovery.
- `READY` means data is eligible for future read-only calculations; it does not
  authorize risk, signing, or execution.
