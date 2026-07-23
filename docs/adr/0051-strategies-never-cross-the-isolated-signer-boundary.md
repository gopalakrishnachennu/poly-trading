# ADR 0051: Strategies Never Cross the Isolated Signer Boundary

- Status: Accepted
- Date: 2026-07-21

## Decision

Strategies can create inert candidates only. They cannot access workload
identity, provider handles or the isolated signer protocol. The security
boundary accepts only exact downstream intents that already carry independent
risk and policy provenance, and it enforces purpose, resource, fixed-point
notional, rate, expiry and dual-control ceilings again.

Phase 3.1 certifies this boundary using fake Vault, KMS and HSM providers. No
secret, key, signature, provider connection or activation authority exists.
Identity revocation is irreversible. Compromise revokes before recovery, and
recovery returns inactive.

## Consequences

- A compromised strategy cannot sign arbitrary content.
- Provider failure cannot bypass risk or signer policy.
- Dual-control evidence is accountability data, not automatic activation.
- Real provider integration must preserve the same fail-closed protocol.
