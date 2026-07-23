# Phase 2.8 Specification: Paired Opportunity Orchestration

## Objective

Compose Phase 2.7 detection, Phase 2.6 proposal validation, and Phase 2.2
scenario risk under one offline deterministic owner, while evaluating both legs
as independently fillable candidates and granting no placement authority.

## Requirements

1. The runtime owns its arbitrage, proposal, and portfolio-risk engines.
2. Callers provide the exact Phase 2.7 command and a candidate-free risk frame;
   they cannot substitute arbitrage, proposal, or candidate decisions.
3. An opportunity must yield exactly two authentic Phase 2.6 candidate
   decisions matching the plan's Up and Down intents.
4. Portfolio risk evaluates both candidates in one Cartesian product with all
   resting orders, terminal outcomes, and shocks.
5. Combined buy collateral and per-token sell inventory capacity are checked
   across both candidates before scenarios run.
6. A multi-candidate risk digest uses a domain distinct from a single-order
   approval and cannot authorize either leg through placement policy.
7. Any ordinary detector no-opportunity or risk `NO_TRADE` is attributable and
   grants no authority. Cross-component substitution or child integrity failure
   halts the owner.
8. Commands are bounded, canonical, content-idempotent, journal-first,
   replayable, checkpoint-verifiable, and digest-stable.
9. No reservation, policy permit, split/merge, signing, credential, network,
   wallet action, or order submission is added.

## Acceptance criteria

- Tests cover authentic three-stage composition, combined fill scenario count,
  combined capacity rejection, candidate and context substitution, detector and
  risk no-opportunity, single-order policy incompatibility, idempotency, sync
  failure, replay/checkpoint equality, and stricter-capital properties.
- A bounded TLA+ model proves ordered detector→two proposals→combined risk,
  independent fill coverage, no single-leg authorization, child-halt
  propagation, one-use evaluation, and absorbing halt.
- Formatting, warnings-denied Clippy, all workspace tests, and all formal models
  pass.

## Exclusions

Capital reservation, atomic execution, leg sequencing, split/merge transactions,
market making, predictive models, credentials, signing, authenticated transport,
and live submission.
