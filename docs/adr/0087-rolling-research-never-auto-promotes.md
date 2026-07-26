# ADR 0087: Rolling research never auto-promotes

- Status: Superseded in part by ADR 0088
- Date: 2026-07-26

## Decision

The rolling BTC-hourly job orchestrates only immutable fixed-point research
artifacts. Under the original decision it could publish `NO_TRADE` or
`PAPER_REVIEW_REQUIRED`. ADR 0088 replaces the manual paper-review requirement
with checksum-bound automatic paper-only registration. It still cannot grant
live authority, open a failed model's final holdout, or access any external
execution capability.

Mutable capture files are represented by newline-complete, checksum-addressed
prefix blobs. A cycle identity is derived from the normalized completed-session
artifact and every historical/policy dependency, so growth in an unfinished
hour cannot create a different completed research cycle.

## Consequences

Scheduled retraining is reproducible and idempotent. A positive research result
may now enter the paper-only registry under ADR 0088; live use remains outside
this decision.
