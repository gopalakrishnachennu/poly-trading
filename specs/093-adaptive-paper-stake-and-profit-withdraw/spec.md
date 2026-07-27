# Specification 093: Adaptive Paper Stake And Profit Withdraw

## Objective

Size each paper bet like a human trader with a fixed $1,000 principal: put more
collateral behind setups that are actually winning, keep weak setups small, and
lock in a withdraw when realized profit reaches 50% of principal so the book can
compound toward the next target without giving the whole gain back.

## Problem

The engine sized every entry at roughly 0.25%–0.50% of bankroll (~$2–$5 on
$1,000). That is too small to make meaningful paper progress, and it ignores
setup quality: a 65% win-rate favourite was staked the same as an unproven or
losing setup.

Operators also need an explicit profit checkpoint. Without one, a green day can
give back the entire gain before anyone notices a target was hit.

## Scope

- Adaptive stake as a percent of tradable bankroll, driven by that setup's own
  closed-trade win rate and sample size from the day ledger.
- Stake tiers (basis points of bankroll):
  - unproven (<5 closes): 200 bps (2%)
  - weak (WR < 45%): 150 bps (1.5%)
  - average (45–55%): 300 bps (3%)
  - good (55–65%): 600 bps (6%)
  - strong (≥65%): 1_000 bps (10%)
- Shadow / experimental setups are capped at half the core tier, max 300 bps.
- High-ask entries still trim stake by 25%.
- Max single-market exposure raised to 1_200 bps; daily loss pause at 2_500 bps
  so larger stakes are not stopped by the old 2% daily cap after one loss.
- Tradable bankroll = principal + realized − withdrawn (floored at a small
  positive minimum so sizing never goes to zero while the worker is healthy).
- When realized − withdrawn ≥ 50% of principal, withdraw exactly that 50%
  checkpoint into `withdrawn_micros`, journal the event in status, and continue
  trading on the reduced tradable bankroll toward the next checkpoint.
- Surface stake tier, tradable bankroll, withdrawn total, and next withdraw
  target in the worker status and Positions GUI.

## Exclusions

- Live trading, credentials, wallets, signing, or Paper Week capital authority.
- Changing setup entry logic or exit rules.
- Kelly sizing from calibrated edge (tiered win-rate is the v1 policy).
- Auto-withdraw of more than the 50% checkpoint in one step.

## Acceptance criteria

1. A setup with ≥5 closes and win rate ≥65% proposes a stake near 10% of
   tradable bankroll (before high-ask trim and exposure cap).
2. A setup with ≥5 closes and win rate <45% proposes a stake near 1.5%.
3. A setup with fewer than 5 closes proposes the unproven 2% tier.
4. Shadow / experimental entries never exceed 3% of tradable bankroll.
5. When cumulative realized minus withdrawn reaches 50% of the $1,000 principal,
   status reports a withdraw of $500 and tradable bankroll falls by that amount.
6. A second identical checkpoint can fire later after another +$500 of net
   realized progress.
7. No floating-point type appears in stake, bankroll, or withdraw paths.
8. Unit tests cover each stake tier, the experimental cap, and the withdraw
   checkpoint.
