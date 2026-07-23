# Phase 4.7 — Core-Backed Paper Execution Bridge

## Objective

Run continuous public-data capture and deterministic paper execution together.
All simulated collateral, risk, reservation, order-lifecycle, settlement and
P&L transitions must be owned by the existing core runtime, never by the
browser or a dashboard-local balance model.

## Scope

The bridge evaluates only complete-set opportunities from authoritative,
fresh BTC/ETH complementary books. It models a paper venue and produces no
credential, private-key, authenticated transport, wallet, transaction, or
external-order capability.

## Acceptance criteria

1. Public capture continues whether a paper opportunity is accepted, rejected,
   partially filled, unknown, cancelled, or settled.
2. A campaign binds one immutable execution policy containing fixed-point
   capital floor, protected backup reserve, per-asset/global limits, fill
   model, latency, fees, slippage and complete-set constraints. No financial
   setting is taken from browser state after campaign start.
3. A simulated pair can enter execution only through the existing strategy,
   risk, reservation and paired execution authorities. The dashboard cannot
   create, alter, approve, reserve, fill, or settle an order.
4. Both legs reserve exact collateral before simulated placement. Partial,
   unknown, delayed, cancel-pending and failed states retain backing until an
   explicit terminal/reconciled transition proves release is safe.
5. Every mock venue event preserves source, receive and strategy-available
   time, has a deterministic identity, and is journaled before mutation.
6. Simulated P&L is distinct from locked and confirmed value. No displayed
   value is spendable unless the core ledger has confirmed it.
7. Restart/replay yields the identical core and dashboard digest. Fault tests
   cover stale feed, one-leg fill, partial fill, unknown outcome, cancel race,
   journal failure, policy mismatch, clock regression and rollover.
8. No external order, wallet, signing, RPC, credential, relayer, split/merge,
   or live authority is introduced.
