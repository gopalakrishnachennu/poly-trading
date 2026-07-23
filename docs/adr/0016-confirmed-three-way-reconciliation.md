# ADR 0016: Gate Readiness on Three-Way Confirmed Truth

## Status

Accepted

## Context

Local intent, CLOB settlement status, the internal ledger, and finalized wallet
balances can disagree. A CLOB match or mined transaction is not final inventory,
and a correct trading model cannot prevent losses caused by posting unconfirmed
assets or overlooking an unexplained wallet difference.

## Decision

Add `settlement-reconciliation` as a read-only deterministic single-writer
kernel. Each trade binds immutable local economics to the documented CLOB
`MATCHED`, `MINED`, `RETRYING`, `CONFIRMED`, and `FAILED` lifecycle. Only
`CONFIRMED` represents successful finality. Terminal trade states are immutable.

Reconciliation compares an atomic bounded view captured from the Phase 2.0
ledger with a caller-supplied finalized chain snapshot. Collateral and every
condition/token balance must match exactly. Confirmed trades must have their
expected ledger command; failed and non-terminal trades must not. A confirmed
trade may remain pending through a configured inclusive grace interval, but
expiry halts.

History regression, same-height block equivocation, ledger-digest equivocation,
unknown trades, changed economics, impossible lifecycle transitions, premature
posting, and asset mismatch are absorbing integrity failures. Commands are
journaled and device-synced before state mutation and rebuilt by strict replay.

## Consequences

- CLOB status can never independently authorize spendable inventory.
- Reconciliation readiness is stronger than feed or session readiness.
- External adapters can be added later without changing deterministic safety
  semantics.
- Automatic repair, authenticated user feeds, RPC, wallets, signing, and order
  submission remain outside this phase.
