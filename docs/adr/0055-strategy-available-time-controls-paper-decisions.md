# ADR 0055: Strategy-Available Time Controls Paper Decisions

- Status: Accepted
- Date: 2026-07-21

## Context

Event time describes the source event and receive time describes capture, but
neither alone proves when a normalized fact was available to a strategy.
Backtests can leak future information if they decide from corrected or delayed
facts before live processing could have exposed them.

## Decision

Phase 3.5 preserves event, receive and strategy-available time independently.
Paper decisions may consume only records whose available time is at or before
the decision cutoff. Walk-forward folds are chronological and non-overlapping,
and the strategy identity is frozen before the final test fold.

Passive fills require queue-volume evidence under three explicit queue cases;
a price touch is insufficient. Conservative outcomes remain mandatory and
unknown execution retains reservations until authentic reconciliation evidence.

## Consequences

Recorded-data results are reproducible and point-in-time valid but may be less
profitable than naive backtests. Local certification still represents neither
real fills nor real profit.
