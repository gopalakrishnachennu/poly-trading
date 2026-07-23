# Phase 2.2 Specification: Scenario Portfolio Risk Gate

## Objective

Add an offline deterministic pre-trade authority that returns auditable
`APPROVE` or non-bypassable `NO_TRADE` from reconciled assets, open-order fill
permutations, terminal outcomes, correlated shocks, haircuts, and hard limits.

## Requirements

1. Every evaluation binds a ready Phase 2.1 reconciliation digest to the exact
   Phase 2.0 ledger digest and rejects stale or equivocal provenance.
2. Open buy orders are exactly backed by reserved cash. Open sell orders are
   exactly backed by reserved token quantities. Candidate orders use only
   available cash or tokens.
3. Each order is evaluated at zero, configured partial, and full fill. Duplicate
   fill quantities are collapsed deterministically.
4. The engine evaluates every Cartesian open-order/candidate fill permutation,
   every binary terminal-outcome permutation, and every configured correlated
   shock profile within a hard scenario budget.
5. Buy cost and fees round upward; sale proceeds round downward; partial fees
   round upward. Arithmetic is checked integer micros only.
6. Reserved cash, available tokens, reserved tokens, and locked tokens use
   separately configured conservative haircuts. Shock-group multipliers can only
   reduce terminal asset value.
7. Every scenario subtracts operational and pending-settlement reserves and
   reports the minimum terminal wealth plus its deterministic witness.
8. Every scenario enforces capital floor, gross exposure, per-condition
   directional exposure, and correlated-group exposure.
9. Missing readiness, stale provenance, reservation mismatch, capacity failure,
   scenario-budget excess, or any risk-limit breach returns `NO_TRADE`.
10. History regression, equal-time reconciliation equivocation, command-ID
    conflict, and impossible internal arithmetic are absorbing integrity halts.
11. Approval is only an immutable risk decision; this crate cannot sign, submit,
    cancel, or otherwise execute an order.
12. Commands are versioned, bounded, content-idempotent, append-and-sync before
    mutation, replayable, checkpoint-verifiable, and digest-stable.

## Acceptance criteria

- Tests cover readiness/staleness/provenance, exact reservation backing, partial
  and simultaneous fills, asymmetric fee rounding, all outcome combinations,
  correlated shocks, each haircut class, capital floor boundaries, every
  exposure limit, candidate capacity, scenario bounds, history equivocation,
  idempotency, rollback, replay, checkpoints, corruption, and journal failures.
- Property tests prove minimum wealth never increases when haircuts worsen or an
  additional adverse shock is included.
- A bounded TLA+ model proves approval implies reconciliation, capital floor,
  exposure limits, scenario completeness, and halt absorption.
- Formatting, denied-warning Clippy, all workspace tests, and all TLC models pass.

## Exclusions

Strategy alpha, price prediction, authenticated exchange access, order state
mutation, credentials, wallet/RPC access, signing, submission, cancellation,
automatic risk-limit changes, and external databases.
