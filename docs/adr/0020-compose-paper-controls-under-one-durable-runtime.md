# ADR 0020: Compose paper controls under one durable runtime

## Status

Accepted for Phase 2.5.

## Decision

The `paper-trading-runtime` crate is the sole writer across the Phase 2.0–2.4
paper path. It owns one accounting ledger, settlement reconciler, portfolio-risk
engine, intent-policy engine, and paper-execution engine. Callers provide
explicit commands, simulated exchange/settlement observations, and finalized
paper chain snapshots; they cannot substitute the runtime's ledger or
reconciliation provenance.

The enforced order is risk approval, exact capital/token reservation, policy
permit, paper execution, unique fill handoff, confirmed ledger posting, and
three-way reconciliation. Paper delayed, live, and terminal observations are
mirrored into policy lifecycle state in the same transactional runtime update.
Any cross-component mismatch or child integrity error halts the entire owner.

Confirmed ledger buys and sells additionally require an unconsumed registered
execution handoff with the same ledger command identity and exact token, side,
quantity, consideration, and fee. Reservations cannot be released while an
order is active or unknown, while it is fully matched, or while a partial-fill
handoff remains unposted.

Pipeline commands are bounded and content-idempotent. The durable wrapper
appends and device-syncs each command before installing its state transition.
Restart strictly replays the journal and verifies optional prefix checkpoints.
Faults before risk, execution, and handoff are explicit durable one-shot events;
an integrity fault is absorbing.

## Consequences

The paper path can be tested and recovered as one auditable state machine rather
than as independently correct components joined by unchecked application code.
It remains fully simulated: there is no strategy alpha, credential, private key,
signature, authenticated client, automatic retry, RPC, wallet action, or live
order submission.
