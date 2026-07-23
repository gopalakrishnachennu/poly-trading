# Phase 2.9 Specification: Transactional Paired Capital Staging

## Objective

Consume an exact Phase 2.8 paired evaluation under the same real ledger view and
reserve both candidate legs transactionally before producing an inert staging
attestation.

## Requirements

1. The runtime owns one Phase 2.8 paired runtime and one Phase 2.0 ledger.
2. The paired command's risk ledger must exactly equal the owned ledger before
   evaluation; callers cannot substitute capital provenance.
3. Only `RISK_ELIGIBLE` paired decisions with exactly two authentic candidates
   may stage capital.
4. Buy legs reserve conservative full cost plus maximum fee. Sell legs reserve
   the exact confirmed token quantity.
5. Both reservations are applied to a cloned ledger and installed together.
   Failure of either leg installs neither reservation.
6. A fully staged record binds the paired decision, both candidates, both
   reservations, and the post-reservation ledger digest.
7. Abort is allowed only before any downstream execution authority exists and
   releases both active reservations transactionally. One-leg release is absent.
8. A staging attestation is not placement policy, signature, split/merge, or
   order submission authority.
9. Commands are bounded, canonical, content-idempotent, journal-first,
   replayable, checkpoint-verifiable, and digest-stable. Child, boundary,
   arithmetic, or durable integrity failure halts the owner.

## Acceptance criteria

- Tests cover exact two-leg buy reservations, paired no-trade, ledger
  substitution, atomic second-leg failure, exact reservation ownership, paired
  abort/release, duplicate abort, command conflict, sync failure,
  replay/checkpoint equality, and capital conservation properties.
- A bounded TLA+ model proves both-or-neither staging, no one-leg release,
  abort-before-authority, no placement authority, child-halt propagation, and
  absorbing halt.
- Formatting, warnings-denied Clippy, all workspace tests, and all formal models
  pass.

## Exclusions

Placement policy, execution sequencing, split/merge transactions, fills,
credentials, signing, authenticated transport, wallet/RPC actions, and live
submission.
