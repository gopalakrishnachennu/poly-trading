# ADR 0053: Chain Truth Requires Multi-Provider Finalized Agreement

- Status: Accepted
- Date: 2026-07-21

## Decision

No single RPC provider is authoritative. Chain readiness requires three
independent providers to agree exactly on chain identity, finalized block/hash
and the complete wallet-state digest while each head remains within its own lag
bound. Pending and mined transactions never create spendable inventory.

A pre-finality reorganization clears readiness and requires new complete
agreement plus no-mutation evidence. A finalized-height hash change or history
regression halts. Collateral, allowance and each CTF token remain separate
signed fixed-point quantities.

## Consequences

- Provider compromise or lag cannot silently become wallet truth.
- Allowance is never counted as collateral.
- Non-final transactions cannot create spendable balances.
- Phase 3.3 can be locally certified without RPC credentials or wallet access.
