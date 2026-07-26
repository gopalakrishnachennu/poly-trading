# ADR 0085: Local hourly poll time is observation time

- Status: Accepted
- Date: 2026-07-26

## Decision

The timestamp written by `capture_windows.py` for a one-hour book is a local
gateway observation timestamp, not an exchange event timestamp. Research
normalization may set event and receive time to that value only when it also
records `LOCAL_GATEWAY_POLL_OBSERVATION` provenance and requires the UP and DOWN
books produced by the same atomic gateway frame.

## Consequences

Local captures can be used causally without inventing unavailable venue time.
They cannot support queue-position claims. Missing legs, conflicting duplicate
observations, invalid depth, and identity disagreement are excluded or rejected
before dataset construction.
