# ADR 0028: Compose Paired Settlement and Accounting Under One Writer

## Status

Accepted for Phase 2.12.

## Decision

The `paired-settlement-runtime` owns Phase 2.11 execution and one Phase 2.1
settlement reconciler. It registers intents only by indexing immutable handoffs
already stored inside its execution child. A caller cannot supply an intent or
ledger view.

Settlement observations retain their documented matched, mined, retrying,
confirmed, and failed meanings. Only a confirmed trade can consume its exact
handoff and post a buy or sell against the stage reservation. The runtime builds
reconciliation frames from the ledger nested below Phase 2.11 and a finalized
paper-chain snapshot. A supplied chain view is evidence, not a repair command.

Residual capital is finalized transactionally across both legs only after both
orders are terminal, every handoff is registered, all trades are terminal,
confirmed trades are posted, failed trades are unposted, and the reconciliation
digest covers the current ledger. Confirmed equal complementary buy inventory
may be moved into a ledger pair lock after reconciliation, but no merge or
spendable proceeds are inferred.

The top-level command boundary is content-idempotent, append-and-sync before
mutation, strictly replayable, and prefix-checkpointed. Any child, provenance,
lifecycle, accounting, reconciliation, release, or durability failure halts the
complete owner.

## Consequences

Matched paper fills cannot become confirmed inventory merely because execution
completed. Paired capital cannot be released while settlement or reconciliation
is ambiguous. The runtime remains offline and contains no signer, credential,
authenticated client, RPC, wallet action, split/merge transaction, automatic
retry, or live submission capability.
