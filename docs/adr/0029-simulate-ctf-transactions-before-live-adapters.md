# ADR 0029: Simulate CTF Transactions Before Live Adapters

## Status

Accepted for Phase 2.13.

## Decision

The `ctf-transaction-runtime` owns Phase 2.12 and models split, merge, and
redemption as explicit deterministic transactions. Split collateral and
redemption tokens enter ledger reservations at request time. Merge inventory
enters a complete-pair lock or binds an existing active lock. Thus requested,
pending, and retrying work cannot expose the same input to another action.

Only monotonic simulated observations can advance a request through pending,
retrying, confirmed, or failed state. Duplicate submission reports are audited
no-ops. Confirmation derives one exact ledger command from stored immutable
request facts; callers cannot provide a posting. Failed split and redemption
release their reservations, while failed merge retains the pair lock for a new
explicit recovery request. No automatic retry exists.

Confirmed split allocates collateral cost deterministically across equal
complementary quantities. Confirmed merge realizes only the payout already
bound by the pair lock. Confirmed redemption consumes reserved outcome tokens
and recognizes only the request's bounded payout and resolution fingerprint.

Commands are append-and-sync before mutation, content-idempotent, strictly
replayable, digest-stable, and prefix-checkpointed. Child or cross-boundary
failure halts the complete owner.

## Consequences

Conversion math, lifecycle, duplicate handling, recovery, and accounting can be
validated without a wallet or chain connection. This phase adds no RPC,
credential, signature, allowance mutation, relayer, authenticated transport,
automatic retry, or live transaction capability.
