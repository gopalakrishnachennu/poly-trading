# ADR 0076: Official Gamma verifies every third-party hourly candidate

- Status: Accepted
- Date: 2026-07-26

## Decision

A revision-pinned third-party catalogue may nominate BTC one-hour market IDs,
but it cannot define market identity, timing, token mapping, rules, or
resolution. Each nominated market must match a preserved official Gamma
response. The official `eventStartTime` defines the opening because the
third-party `start_ts` field was observed to contain market creation time.

## Consequences

External historical data can increase independent resolved-session coverage
without silently contaminating labels or session boundaries. Any missing hour
or official/external disagreement rejects the full requested publication.
These artifacts remain supplemental research evidence and create no model,
paper, risk, capital, signing, wallet, or execution authority.
