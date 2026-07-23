# Phase 2.14 Specification: Unified Offline Paired-Trading Runtime

## Objective

Own the complete Phase 2.8 through Phase 2.13 path behind one deterministic
offline command boundary. Callers provide market/risk inputs and simulated
external observations, but cannot construct child commands, ledger provenance,
permits, handoff posting identities, or reconciliation frames.

## Requirements

1. One single writer owns one Phase 2.13 runtime and therefore every nested
   opportunity, accounting, policy, execution, settlement, and conversion
   authority.
2. Funding, reconciliation, evaluation/staging, mode observation,
   authorization/submission, cancellation, paper observations, handoff
   registration, settlement, confirmed posting, pair locking, finalization,
   and conversion lifecycle are explicit top-level commands.
3. Evaluation derives the current reconciliation gate and exact nested-ledger
   risk view. A caller cannot supply either provenance object.
4. Authorization and submission are one transactional top-level operation.
   The runtime extracts and consumes the exact internally issued permit.
5. Confirmed posting derives its ledger command identity from the stored order
   handoff selected by stage, leg, and handoff index.
6. Every child command and idempotency key is deterministically derived from
   the immutable top-level command identity and substep.
7. Multi-step commands install all child mutations or none. Any child,
   identity, history, arithmetic, or injected-boundary failure halts the owner.
8. Commands are bounded, canonical, content-idempotent, journal-first,
   replayable, checkpoint-verifiable, and use fixed-point arithmetic only.
9. Restart recovery reproduces the complete nested state digest. Multi-hour
   no-trade, settlement, conversion, and restart profiles conserve capital and
   leave no inaccessible asset without an attributable record.
10. No command can sign, authenticate, call RPC, access a wallet, submit a live
    order or transaction, retry automatically, or activate paid infrastructure.

## Acceptance criteria

- Tests cover provenance derivation, stage-to-submission composition, handoff
  identity derivation, confirmed settlement, split/merge finality, duplicate
  and conflicting command identity, transactional injected failure, journal
  recovery, checkpoints, and multi-hour capital conservation.
- Property tests prove no-trade hours cannot change capital and stricter
  command replay cannot create a second mutation.
- A bounded TLA+ model proves ordered authority, all-or-none submission,
  confirmed-only posting, conversion backing, no live authority, and absorbing
  halt.
- Formatting, warnings-denied Clippy, all workspace tests, and every formal
  model pass.

## Exclusions

Credentials, signing, authenticated APIs, RPC, wallets, relayers, live orders,
live CTF transactions, automatic retry, predictive models, and distributed
production infrastructure.
