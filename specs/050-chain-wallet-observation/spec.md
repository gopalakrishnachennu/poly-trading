# Phase 3.3 Specification: Blockchain and Wallet Observation

## Objective

Build deterministic read-only multi-provider chain truth for chain identity,
head/finality, pre-finality reorganizations, collateral, allowances, CTF token
balances and transaction lifecycle without RPC mutation or wallet authority.

## Scope

- Exact current Phase 3.2 local venue report binding.
- Three independent credentialless read-only RPC provider contracts.
- Exact chain ID, genesis, wallet, collateral-token, CTF and exchange subjects.
- Provider head and finalized-block observations with bounded head lag.
- Canonical wallet snapshots containing signed integer collateral, allowance and
  sorted per-token balances.
- Pending, mined, finalized and failed transaction observations.
- Exact multi-provider agreement before a snapshot becomes authoritative.
- Pre-finality reorganization invalidation and explicit recovery.
- Isolated provider-disagreement, stale-head and chain-mismatch failure fixtures.
- Journal replay, checkpoints and create-new local-certification reports.

## Exclusions

- RPC credentials, private keys, wallet signing or transaction submission
- Token approvals, transfers, withdrawals, split/merge or redemption mutation
- Single-provider authoritative truth
- Treating pending/mined transactions as finalized assets
- Live-environment certification without real provider evidence

## Acceptance criteria

- Stale, substituted, incomplete, live-certified or authority-bearing Phase 3.2
  evidence fails registration.
- Exactly three unique read-only provider contracts bind the same chain and
  expose no credential, signer, wallet or mutation capability.
- Agreement frames contain one observation from every provider and exactly
  agree on finalized block/hash and wallet-state digest.
- Finalized history is monotonic and immutable. Regression or same-height hash
  equivocation halts.
- Head lag is bounded independently for every provider. One fresh provider
  cannot hide another stale provider.
- Collateral, allowance and token balances use signed fixed-point integers and
  remain distinct. Pending/mined transactions are never spendable.
- Pre-finality reorg clears current readiness and requires a complete fresh
  agreement plus no-mutation evidence; finalized-block reorg is impossible.
- Provider disagreement and chain mismatch are halt-class fixtures; stale head
  is denied. None may contribute authoritative state.
- Completion covers agreement, head/finality advance, reorg, disagreement,
  staleness, chain mismatch, balance, allowance and transaction lifecycle.
- Reports grant zero RPC, wallet, signing, deployment, trading or submission
  authority and distinguish local from live-environment certification.
- Tests and TLA+ cover agreement, finality monotonicity, reorg invalidation,
  wallet separation, transaction finality, no-mutation and absorbing halt.
