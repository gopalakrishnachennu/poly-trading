# Phase 2.5 Specification: End-to-End Paper-Trading Runtime

## Objective

Compose accounting, reconciliation, portfolio risk, intent policy, and paper
execution into one deterministic, journal-first, restartable orchestration
authority with scripted fault injection and multi-hour soak verification.

## Requirements

1. The runtime owns exactly one Phase 2.0–2.4 engine instance and applies all
   cross-component commands through a single writer.
2. Risk requests must equal the runtime's current reconciliation gate and exact
   categorized ledger view; caller-supplied provenance cannot substitute them.
3. Approved candidate orders must receive an exact matching collateral/token
   reservation before placement policy can authorize them.
4. Placement policy must consume the runtime's exact last risk decision;
   execution must consume the runtime's exact last policy decision.
5. Paper delayed/live/terminal observations update the corresponding policy
   lifecycle transactionally.
6. Reconciliation intents may enter only from an exact accepted paper-execution
   handoff and may be registered once.
7. Reconciliation frames must use the runtime's actual ledger view; settlement
   observations remain explicit simulated inputs.
8. Fault points before risk, execution, and handoff are durable, deterministic,
   one-shot, and recoverable. Integrity-fault injection is an absorbing halt.
9. Any child-engine integrity failure halts the pipeline. A healthy component
   cannot hide another component's halt.
10. Commands are bounded, versioned, content-idempotent, journaled and synced
    before mutation, replayable, checkpoint-verifiable, and digest-stable.
11. Multi-hour scripted soaks cover approvals, no-trades, rejections, unknown
    recovery, partial/full fills, reservations, settlement handoff, and restart.
12. No strategy alpha, credential, signature, authenticated transport, RPC,
    wallet action, automatic retry, or live submission is added.

## Acceptance criteria

- Tests cover exact cross-component provenance, reservation ownership/amount,
  policy/execution decision substitution, handoff uniqueness, ledger-frame
  substitution, one-shot faults, integrity halt, online/replay/checkpoint
  equality, sync failure, complete confirmed-fill reconciliation, and multi-hour
  scripted soak.
- A bounded TLA+ model proves ordered risk→reserve→policy→execution→handoff,
  handoff uniqueness, child-halt propagation, one-shot fault behavior, and
  absorbing pipeline halt.
- Formatting, warnings-denied Clippy, all workspace tests, and all formal models
  pass.

## Exclusions

Predictive strategies, live market adapters, API keys, signing, authenticated
submission, automatic retries, databases, brokers, Kubernetes, and paid services.
