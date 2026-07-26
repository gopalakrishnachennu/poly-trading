# Phase 4.8 Specification: BTC Hourly Intraday Research Dataset

## Objective

Create a read-only deterministic research builder for BTC one-hour sessions.
It derives conservative executable round-trip labels from captured L2 tape and
grants no strategy, model, or paper runtime any authority.

## Scope

- Parse exported CLOB L2 JSONL tape.
- Select only immutable BTC markets with an exact one-hour duration.
- Preserve source event, local receive, and strategy-available timestamps.
- Price an immediate buy from ask depth and a future sell from bid depth using
  fixed-point integer arithmetic.
- Split samples by whole chronological market sessions, never by row.
- Write a digest-bound manifest and an explicit NO_MODEL eligibility result
  when evidence is insufficient.

## Exclusions

- Strategy selection, model training, online learning, paper-order changes,
  credentials, wallets, signing, authenticated transport, and live orders.
- Passive maker-fill claims. Public L2 cannot establish an exact hypothetical
  queue position.

## Acceptance criteria

- Invalid identity, timestamp ordering beyond the configured source-clock lead,
  book side, price, quantity, or
  incomplete executable depth rejects the frame; it is never treated as a fill.
- Entry and exit costs use integer micros with conservative rounding; no money,
  quantity, fee, or return uses binary float.
- Every emitted frame includes event, receive, strategy-available time, source
  sequence, time-to-expiry, session progress, source-clock lead, and policy
  digest.
- A sample only uses information available at its own strategy-available time;
  labels are strictly later observations.
- Train, validation and test sets contain disjoint whole chronological BTC
  sessions.
- Fewer than the configured number of completed BTC sessions produces an
  explicit NO_MODEL report, not a trainable dataset.
- Output is derived, local, and non-authoritative.
