# Phase 2.4 Specification: Deterministic Paper Execution Lifecycle

## Objective

Add a simulated/replayed execution gateway that consumes exact Phase 2.3
placement and cancellation permits, models complete order and cancel-race
lifecycle state, and produces immutable Phase 2.1 reconciliation handoffs for
paper fills without any live exchange capability.

## Requirements

1. A paper submission requires an authentic Phase 2.3 `PERMIT` for `PLACE`, the
   exact placement-request fingerprint, matching order ID, and an unexpired
   authorization.
2. A paper cancel request requires an authentic Phase 2.3 `PERMIT` for `CANCEL`
   and the exact cancel-request fingerprint.
3. Order states distinguish `SUBMITTED`, `DELAYED`, `ACKNOWLEDGED`, `LIVE`,
   `PARTIALLY_MATCHED`, `CANCEL_PENDING`, `UNKNOWN`, `FULLY_MATCHED`, `CANCELED`,
   and `REJECTED`.
4. Every simulated exchange observation preserves source sequence, event time,
   receive time, exchange order ID, and immutable external facts.
5. Unknown submission/cancel results remain exposure-bearing until a newer
   authoritative observation resolves them. Unknown never means rejected.
6. Cancel-pending orders may still partially or fully match. Cancel acceptance,
   rejection, and match arrival are explicit race outcomes.
7. Delayed orders may be rejected after the delay and cannot become live before
   their release timestamp.
8. Partial/full matches carry unique fill IDs, delta and cumulative quantity,
   consideration, fee, and an expected ledger command ID. Cumulative arithmetic
   is exact, bounded by the original order, and conservative against its limit.
9. Each accepted fill emits exactly one immutable `TradeIntent` handoff for the
   settlement reconciler. Paper matches remain unconfirmed and unspendable.
10. Rejections classify permanent, restart, rate-limit, balance/allowance,
    delayed-check, and unknown retry causes without automatically retrying.
11. Command/source regression, equivocation, duplicate fill IDs, impossible
    lifecycle movement, changed exchange order ID, overflow, and durable
    corruption are absorbing halts.
12. Commands are bounded, versioned, content-idempotent, journaled and synced
    before mutation, replayable, checkpoint-verifiable, and digest-stable.
13. The crate contains no credentials, signature, HTTP/WebSocket client, RPC,
    wallet, actual submission, or automatic retry capability.

## Acceptance criteria

- Tests cover policy tampering, request substitution, permit expiry/replay,
  delayed acknowledgement/rejection/release, unknown recovery, every match
  boundary, cancel-before-fill, fill-before-cancel, cancel rejection, full-fill
  races, exchange-ID mutation, source regression/equivocation, retry classes,
  reconciliation handoff exactness, idempotency, rollback, replay, checkpoints,
  corruption, and sync failure.
- Property tests prove cumulative fill quantity never exceeds the original order
  and every handoff quantity sums exactly to accepted cumulative quantity.
- A bounded TLA+ model proves unknown state is non-terminal, cancel-pending may
  still fill, full fill/cancel/reject are terminal, every fill creates one
  handoff, and halt is absorbing.
- Formatting, warnings-denied Clippy, all workspace tests, and every TLC model
  pass.

## Exclusions

Live order submission, API credentials, signing, network adapters, exchange
authentication, automatic retry, production queue-position simulation,
strategies, and actual ledger posting.
