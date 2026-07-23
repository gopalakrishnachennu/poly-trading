# ADR 0018: Risk-approved order intents require an independent offline policy

## Status

Accepted for Phase 2.3.

## Decision

A Phase 2.2 `APPROVE` is necessary but insufficient for an order intent. The
offline `order-intent-policy` authority verifies the decision digest, exact
candidate-order fingerprint, approval age, one-time use, current exchange mode,
and a separately supplied signer-policy frame before emitting `PERMIT`.

New exposure is permitted only in `NORMAL`, or in `POST_ONLY` when the intent is
both post-only and non-marketable. `UNKNOWN`, `RESTARTING`, `CANCEL_ONLY`,
`TRADING_DISABLED`, and `RECOVERING` deny placement. Cancellation is permitted
in reduction-safe modes, except for unknown/restarting state, terminal orders,
or an explicit delayed uncancellable interval.

Signer policy is inert data. It constrains venue, exchange contract, exact token
allowlist, quantity, price, notional, maker/taker capability, and validity
window. This component has no private key and cannot create a signature or send
a request.

Commands are content-idempotent, journaled and device-synced before mutation,
strictly replayed, and checkpoint-verified. Command conflict, mode/clock history
failure, impossible lifecycle movement, overflow, or durable corruption halts.

## Consequences

Risk-approved orders cannot be altered or replayed downstream. Restart and
maintenance behavior is explicit and auditable. Delayed orders cannot be
treated as live or cancellable before their exact boundaries. A later signing
service must re-verify these policy facts independently; this phase does not
authorize production signing or submission.
