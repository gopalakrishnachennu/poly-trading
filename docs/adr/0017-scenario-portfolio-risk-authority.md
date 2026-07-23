# ADR 0017: Scenario portfolio risk is the non-bypassable approval authority

## Status

Accepted for Phase 2.2.

## Decision

Every proposed order is evaluated against a digest-bound, fresh Phase 2.1
reconciliation gate and the corresponding categorized Phase 2.0 ledger view.
The deterministic kernel enumerates the bounded Cartesian product of zero,
partial, and full fills for every resting order and the candidate, all terminal
binary outcomes, and every configured correlated-shock profile.

`APPROVE` is emitted only when every enumerated state preserves the configured
capital floor and gross, condition, and shock-group exposure limits. All normal
risk failures emit an attributable `NO_TRADE`. History regression, source
equivocation, idempotency conflict, arithmetic overflow, and durable-integrity
failure are absorbing halts.

Reserved cash and token inventory must exactly equal the worst-case backing of
resting orders. A candidate may consume only available assets. Cash and each
token category receive explicit conservative haircuts; inaccessible and
operational reserves are deducted separately.

## Consequences

The engine can safely decline an order but cannot sign or submit one. Scenario
growth is rejected before enumeration when it exceeds the configured or hard
bound. Replay and checksummed checkpoints reproduce the same decision and
state digest. Predictive probability, expected hedge availability, rebates,
and unconfirmed balances cannot improve an approval.
