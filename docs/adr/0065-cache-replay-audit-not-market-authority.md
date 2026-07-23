# ADR 0065: Cache paper replay audits separately from market status

- Status: Accepted
- Date: 2026-07-23

## Decision

The terminal's market projection and paper campaign status are lightweight
operational reads. Full paper replay verification instead reads every record in
the checksummed journal and is therefore treated as a slower audit operation.

The gateway shares a bounded fifteen-second replay-audit cache among dashboard
tabs. The browser polls paper status at the public-feed cadence but obtains the
full report and research-export status only at the audit cadence. Audit output
includes the verification timestamp. During initial loading or audit failure,
the terminal labels the state as reconciling/audit unavailable rather than
claiming an idle or healthy campaign.

## Consequences

- Multiple operator tabs cannot repeatedly contend with the recorder by reading
  the complete growing journal once per second.
- A cached audit never changes market readiness or grants any execution,
  capital, signing, or trading authority.
- Public market data remains fail-closed: missing, stale, malformed or
  inconsistent feeds still clear eligibility to `NO_TRADE` immediately.
