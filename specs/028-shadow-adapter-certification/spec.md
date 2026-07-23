# Phase 2.15 Specification: Shadow-Adapter Certification

## Objective

Deterministically certify an adapter deployment profile using immutable public
interface contracts, recorded venue fixtures, signer-policy dry runs, synthetic
wallet/allowance/gas/relayer observations, public eligibility attestations, and
mandatory failure-response coverage. Certification grants no runtime authority.

## Requirements

1. The authority stores one immutable adapter contract binding venue, public
   hosts, chain/contracts, schema, required regions, timing bounds, and rules.
2. Recorded fixtures are payload-digest-bound, monotonically sequenced, and
   cover restart, post-only, cancel-only, taker-delay, tick-change, rate-limit,
   unknown-order, settlement-retry, and heartbeat-loss behavior.
3. Signer dry runs operate on policy and intent data only. They emit permit or
   denial facts but cannot load a key, create signature bytes, or submit.
4. Certification requires a permitted baseline dry run and explicit denial
   coverage for contract, token, quantity, and expiry violations.
5. Every required deployment region needs a fresh eligible public attestation
   bound to an opaque egress fingerprint and source digest.
6. Synthetic operational observations separately represent collateral,
   allowance, gas, relayer availability, and queue depth without wallet access.
7. Mandatory simulated failures map to fixed non-exposure actions: deny new
   exposure, retain backing/reconcile, back off without automatic retry, or
   retain unconfirmed value.
8. Missing, stale, unhealthy, or incomplete evidence yields `NOT_CERTIFIED`.
   Regression, equivocation, identity substitution, arithmetic failure, or
   durable corruption yields an absorbing halt.
9. Commands are bounded, canonical, content-idempotent, journal-first,
   replayable, checkpoint-verifiable, and deterministic.
10. The phase contains no credential, secret, private key, signature, API
    authentication, network client, RPC, wallet, relayer client, order, or
    transaction submission capability.

## Acceptance criteria

- Tests cover complete certification, every missing/stale gate, signer permit
  and denial coverage, fixture history, eligibility identity, allowance/gas/
  relayer failures, mandatory failure actions, idempotency, sync failure,
  journal replay, and checkpoint recovery.
- Property tests prove reducing allowance or gas cannot turn a denial into a
  certification and exact command replay cannot duplicate evidence.
- A bounded TLA+ model proves complete-evidence certification, failure-action
  safety, no authority, and absorbing halt.
- Formatting, warnings-denied Clippy, all workspace tests, and every formal
  model pass.

## Exclusions

Real credentials, signing, API authentication, network calls, RPC, wallets,
allowance transactions, gas estimation calls, relayer calls, orders, and live
transactions.
