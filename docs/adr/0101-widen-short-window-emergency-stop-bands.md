# ADR 0101: Widen the 5m/15m emergency-stop bands

- Status: Accepted
- Date: 2026-07-28

## Decision

Widen `emergency_stop_lo_micros` / `emergency_stop_hi_micros` in
`WindowConfig::for_window`:

- 5m: 6/8 points -> 8/10 points (now matches the prior 15m band)
- 15m: 8/10 points -> 10/12 points

1h is unchanged.

## Evidence

Across 48h of paper trading (all setups) and again in the ~4h window
immediately after disabling three repeat-negative setups (ADR-level policy
version 2), `EMERGENCY_PRICE_STOP` was the single largest cross-setup cost:

- 48h: 256 fires, -$848, 0% win by definition (it is a forced loss exit).
- Post-disable (~4h, 5 remaining setups): 19 fires, -$41 — a larger share
  of total losses than before, confirming the effect is not specific to
  the disabled setups.

A 6-8 point (cent) adverse move against a token typically priced 0.58-0.78
is within ordinary noise for a five-minute contract; the stop was closing
positions before they had a chance to resolve favourably.

## Consequences

Positions can now drift further against entry before the stop forces an
exit, trading a larger per-trade worst case for materially fewer premature
exits. The hard "hi" threshold (forced exit regardless of momentum) still
exists at every tier, so risk remains bounded, just at a wider band. This
must be re-measured the same way (fire count, win%, P&L attributable to
this reason) over the next 24-48h of paper trading before considering any
further widening.
