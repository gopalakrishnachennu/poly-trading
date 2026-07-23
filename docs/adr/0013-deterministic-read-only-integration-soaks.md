# ADR 0013: Gate Integration with Deterministic Multi-Hour Soaks

## Status

Accepted

## Context

The feed, replay, supervision, session, and durable runtime bricks were tested
independently. Hourly rollover failures can still emerge only when those state
machines interact over multiple BTC and ETH windows, delayed evidence, degraded
feeds, checkpoints, and restart.

## Decision

Add an `integration-daemon` owner that performs exactly three ordered actions:
evaluate cross-feed supervision, construct the exact session frame, then apply
it through the journal-first durable runtime. Production replay adapters never
invent unavailable books or candles.

Add a bounded deterministic soak generator for consecutive BTC and ETH hours.
It creates validated identities, integer fixed-point books and oracle candles,
monotonic feed state, exact rollovers, and final evidence. Explicit scheduled
faults operate either before supervision or after supervision; integrity faults
halt while ordinary availability faults may recover.

The CLI currently exposes synthetic soak and strict recovery. External live
orchestration remains disabled until the eligible-network smoke gate passes.

## Consequences

- Identical plans produce identical durable reports and digests.
- Multi-hour behavior is reproducible without relying on external timing.
- Fault tests identify the exact safety layer being exercised.
- This is not permission to trade and contains no authenticated capability.
