# ADR 0097: Adaptive Stake Follows Setup Win Rate

## Status

Accepted

## Context

Paper bets were sized at a flat ~0.25%–0.50% of a $1,000 bankroll. That produced
~$2–$5 stakes regardless of whether a setup was printing 65% winners or losing
money. An operator trying to grow a $1,000 book toward a 50% profit withdraw
cannot do it at that size, and treating every setup as equal is not how a
careful human sizes risk.

## Decision

- **Stake follows evidence.** Each entry's risk budget is a basis-point slice of
  tradable bankroll chosen from that setup's own day win rate and sample size
  (Spec 093 tiers: 1.5% / 2% / 3% / 6% / 10%).
- **Unproven stays small, not tiny.** Fewer than five closes uses 2% — enough to
  learn, not enough to dominate the book.
- **Winners earn size.** Strong setups (≥65% with enough closes) may risk up to
  10% of tradable bankroll. Weak setups are cut to 1.5%.
- **Shadow stays capped.** Experimental / 1h shadow entries take at most half of
  the core tier and never more than 3%.
- **Withdraw at +50%.** When realized profit net of prior withdrawals reaches
  50% of the $1,000 principal, the runtime books a $500 paper withdrawal and
  continues on the reduced tradable bankroll. The checkpoint can repeat.
- **Tradable bankroll is shared.** Every engine sizes against
  `principal + realized − withdrawn`, not a private copy of $1,000 per market.

## Consequences

Stakes become large enough for meaningful paper P&L, and capital concentrates on
setups that are actually working. A cold restart still sizes unproven until the
day journal is replayed into the ledger. Larger stakes require a higher daily
loss pause (2.5%) so a single stop does not freeze the book; operators must
still treat this as paper evidence, not a live mandate.
