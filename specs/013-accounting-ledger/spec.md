# Phase 2.0 Specification: Deterministic Accounting Ledger

## Objective

Add an offline, deterministic double-entry accounting and capital-state kernel
without introducing exchange connectivity, credentials, wallets, strategies, or
order authority.

## Requirements

1. All collateral and token quantities use checked integer micros; floating
   point is forbidden.
2. Every transaction balances independently for collateral and every outcome
   token before any state mutation.
3. Operational asset and cost accounts cannot become negative.
4. Collateral and token reservations have one immutable owner, support partial
   confirmed consumption, and can be released or fully consumed only once.
5. Matched, mined, pending, retrying, or otherwise unconfirmed activity has no
   posting command and cannot create spendable inventory.
6. Confirmed buys, sells, pair locks, and merges post fees, inventory cost,
   revenue, cost of goods sold, cash, and token movements atomically.
7. Cost allocation uses deterministic conservative integer rounding; the final
   disposal consumes the exact remaining cost.
8. Realized net P&L, locked P&L, paid fees, liquid cash, reserved cash, and
   unreserved/reserved tokens are distinct snapshot fields.
9. Repeating an identical command ID is a no-op; reusing an ID for different
   bytes is an absorbing integrity halt.
10. Durable operation is append-and-device-sync before state mutation. Replay
    requires contiguous sequences and exact payload validation.
11. Prefix checkpoints bind a journal sequence to the complete ledger digest.
12. Conservation and reservation invariants are checked after every accepted
    command and during recovery.

## Acceptance criteria

- Tests cover balance rejection, atomic rollback, overdraft rejection, partial
  reservation consumption, release, buy/sell fees, conservative cost basis,
  locked/merged P&L, duplicate commands, conflicting IDs, overflow, replay,
  checkpoint mismatch, corruption, and journal-first failure poisoning.
- Property tests demonstrate per-asset conservation and reservation backing.
- A bounded TLA+ model checks double-entry conservation, reservation backing,
  idempotency, and absorbing halt.
- Formatting, denied-warning Clippy, all workspace tests, and all TLC models
  pass.

## Exclusions

Exchange/user feeds, matching state, blockchain reconciliation, wallets,
allowances, split/merge transaction submission, credentials, signing, orders,
portfolio risk, strategy logic, and external databases.
