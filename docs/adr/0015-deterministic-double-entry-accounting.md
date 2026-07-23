# ADR 0015: Use Per-Asset Double-Entry Accounting

## Status

Accepted

## Context

Capital reservations, partial fills, fees, token inventory, complete pairs, and
settlement can appear locally consistent while silently creating or losing value
if maintained as unrelated counters. Cross-unit postings also cannot balance a
collateral dollar directly against a token because they are different assets.

## Decision

Add `accounting-ledger` as an offline single-writer kernel. A transaction is a
set of signed fixed-point postings whose deltas sum to zero independently for
collateral and for every outcome token. Available, reserved, inventory-cost,
fee, revenue, cost-of-goods-sold, locked-token, and locked-cost accounts remain
separate. Accounts representing controlled assets or deferred cost may never be
negative.

Reservations transfer existing value and have immutable ownership. Only
commands explicitly named as confirmed fills or confirmed merges can consume
reservations or recognize new inventory/cash. Pending exchange and blockchain
states intentionally have no accounting command in this phase.

Command IDs are content-bound. Exact duplicates are no-ops; conflicting reuse
is an absorbing halt. Durable application records and device-syncs the canonical
command before state mutation, and restart rebuilds state by strict replay.

## Consequences

- Conservation is locally checkable after every transaction.
- Token quantities never balance against collateral values.
- Locked profit remains inaccessible and separate from realized profit.
- Fees and deterministic cost allocation are auditable.
- Reconciliation and risk can later consume this ledger without giving it order
  or signing authority.
