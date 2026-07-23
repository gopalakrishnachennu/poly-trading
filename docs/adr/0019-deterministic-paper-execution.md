# ADR 0019: Execution lifecycle is proven in paper replay before live adapters

## Status

Accepted for Phase 2.4.

## Decision

The `paper-execution` kernel consumes only authentic Phase 2.3 placement and
cancellation permits bound to their exact request fingerprints. It models
caller-supplied exchange observations without possessing any transport or
credential capability.

Orders distinguish submitted, delayed, acknowledged, live, partially matched,
cancel pending, unknown, fully matched, canceled, and rejected states. Unknown
is exposure-bearing and non-terminal. A cancel-pending order can still match;
fill, cancel acceptance, and cancel rejection are explicit race outcomes.

Every fill carries unique delta and cumulative quantity, consideration, fee,
and expected ledger command identity. Accepted cumulative values must equal the
prior accepted values plus the delta, remain within the original order and fee
bounds, and respect the limit price conservatively. Each fill creates exactly
one immutable Phase 2.1 `TradeIntent` handoff. It remains unconfirmed and
unspendable until the existing settlement and reconciliation controls succeed.

Source sequence/time regression, exchange-order identity change, duplicate fill
identity, impossible transition, arithmetic failure, command conflict, or
durable corruption halts. Commands are journaled and device-synced before
mutation, replayed strictly, and verified against prefix checkpoints.

## Consequences

Cancel races, delayed rejection, ambiguous results, and partial fills can be
tested deterministically before an authenticated adapter exists. Retry classes
are descriptive only; this phase never retries automatically. Nothing in this
crate can sign, authenticate, connect, or submit a real order.
