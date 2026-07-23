# ADR 0030: Unify Offline Paired-Trading Ownership

## Status

Accepted for Phase 2.14.

## Decision

A new `unified-paired-trading-runtime` owns Phase 2.13 and is the only public
orchestration boundary for the complete offline paired path. Its commands carry
domain inputs and simulated observations, never nested child commands.

The owner derives current ledger and reconciliation provenance for evaluation,
all child command identities, exact placement permits, and confirmed-posting
ledger identities. Authorization and submission occur transactionally in one
top-level transition. Any substep failure installs no child state and halts the
owner. Journal replay persists the top-level command stream, not an externally
assembled stream of child commands.

Read-only nested snapshots remain available for audit. Generic Phase 2.13
parent commands remain an implementation detail below this owner and are not
accepted by the new command language.

## Consequences

Cross-crate composition cannot be reordered or substituted by a Phase 2.14
caller. Recovery has one authoritative digest and one command history. The
owner remains simulated: it adds no signer, credential, authenticated client,
RPC, wallet, relayer, automatic retry, or live order/transaction capability.
