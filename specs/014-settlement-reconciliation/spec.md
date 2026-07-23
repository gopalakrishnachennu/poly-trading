# Phase 2.1 Specification: Settlement Lifecycle and Reconciliation

## Objective

Add a read-only deterministic kernel that reconciles local trade expectations,
CLOB-reported settlement state, the Phase 2.0 ledger, and finalized blockchain
balances without adding authenticated adapters or execution authority.

## Requirements

1. Trade facts bind an immutable local intent, order, condition, token, side,
   quantity, consideration, fee, and expected ledger command.
2. CLOB settlement follows only the documented graph: `MATCHED`, `MINED`,
   `RETRYING`, `CONFIRMED`, and `FAILED`. Terminal states are immutable.
3. `MATCHED`, `MINED`, and `RETRYING` inventory is never confirmed or spendable.
4. Mined and confirmed observations require a transaction hash. Trade facts
   cannot change across updates.
5. Finalized blockchain snapshots bind an immutable chain, wallet, block number,
   block hash, collateral balance, and exact token balances.
6. Block regression, equal-height hash equivocation, timestamp regression,
   terminal trade mutation, impossible status transition, unknown trade, and
   command-ID conflict are absorbing integrity failures.
7. A reconciliation frame captures a real Phase 2.0 ledger view and one
   finalized chain snapshot. Collateral and every token must match exactly.
8. A successful confirmed trade must have its expected ledger command. A failed
   or non-terminal trade must not have that command.
9. A confirmed trade missing its ledger command is pending only through an
   inclusive configured grace interval; expiry is an integrity halt.
10. Readiness is possible only when every registered trade is terminal, all
    confirmed trades are posted, all failed trades are unposted, ledger and
    chain assets agree, and neither source is halted.
11. Commands are bounded, versioned, content-idempotent, append-and-sync before
    mutation, strictly replayable, and checkpoint-verifiable.
12. Snapshots expose source provenance, counts, discrepancy reason, readiness,
    and a stable complete-state digest.

## Acceptance criteria

- Tests cover every legal and illegal lifecycle edge, terminal immutability,
  unknown trades, immutable facts, transaction hashes, grace boundaries,
  ledger-before-confirmation, failed-but-posted, cash/token divergence, chain
  regression/equivocation, ledger regression/equivocation, command conflicts,
  rollback, replay, checkpoints, corruption, and journal failures.
- Property tests cover lifecycle monotonicity and exact asset comparison.
- A bounded TLA+ model proves non-terminal non-spendability, readiness,
  reconciliation equality, terminal immutability, and halt absorption.
- Formatting, denied-warning Clippy, all workspace tests, and all TLC models pass.

## Exclusions

Authenticated user-channel networking, REST polling, RPC clients, wallet access,
signing, order creation/cancellation, ledger posting, automatic repair,
strategies, portfolio risk, and external databases.
