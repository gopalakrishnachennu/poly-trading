# Phase 2.3 Specification: Exchange Mode and Order-Intent Policy

## Objective

Create an offline deterministic policy authority that converts an exact,
unexpired Phase 2.2 approval into a bounded paper placement authorization and
governs cancellation eligibility through explicit exchange modes and delayed,
uncancellable order windows.

## Requirements

1. Placement binds the exact order fingerprint, authentic risk-decision digest,
   signer-policy digest, exchange-mode observation, venue, contract, and time.
2. Only `APPROVE` with `AllLimitsSatisfied` can authorize placement. Risk
   decisions and signer policies expire at inclusive/exclusive exact boundaries.
3. Exchange modes are `UNKNOWN`, `NORMAL`, `RESTARTING`, `POST_ONLY`,
   `CANCEL_ONLY`, `TRADING_DISABLED`, and `RECOVERING`.
4. `NORMAL` permits policy-compliant maker or taker placement. `POST_ONLY`
   permits only non-marketable post-only placement. No other mode permits new
   exposure.
5. Cancels may be authorized in `NORMAL`, `POST_ONLY`, `CANCEL_ONLY`,
   `TRADING_DISABLED`, or `RECOVERING`, but never before an order exists, after
   terminal state, or inside its explicit uncancellable interval.
6. `RESTARTING` and `UNKNOWN` authorize neither placement nor cancellation;
   recovery requires a newer explicit mode observation.
7. Delayed orders are tracked separately from live orders. Release before the
   configured delay or lifecycle mutation after terminal state is an integrity
   halt.
8. Signer policy is data only: it constrains exact venue, contract, token set,
   quantity, price, notional, maker/taker capability, and validity interval.
9. Reused risk approvals, placement IDs, or cancel IDs cannot authorize twice.
   Exact duplicate commands are idempotent; conflicting reuse halts.
10. Mode sequence/time regression, equal-sequence equivocation, invalid state
    transition, arithmetic overflow, or durable corruption is an absorbing halt.
11. Policy decisions are immutable `PERMIT`/`DENY` audit records. This phase has
    no key, signature, authenticated API, network request, or order submission.
12. Commands are versioned, bounded, journaled and device-synced before state
    mutation, replayable, checkpoint-verifiable, and digest-stable.

## Acceptance criteria

- Tests cover every exchange mode, post-only/marketable combinations, risk and
  order fingerprint tampering, approval/policy/mode expiry, venue/contract/token
  allowlists, quantity/price/notional limits, approval replay, delayed release,
  uncancellable boundaries, terminal mutation, history equivocation, exact
  idempotency, replay, corruption, checkpoints, and sync failure.
- Property tests prove a stricter signer quantity/notional limit cannot turn a
  denied placement into a permit.
- A bounded TLA+ model proves placement requires risk approval, fresh policy,
  permitted mode, and no approval replay; cancel permission respects delayed
  uncancellable windows; halt is absorbing.
- Formatting, warnings-denied Clippy, all workspace tests, and every TLC model
  pass.

## Exclusions

Credentials, private keys, cryptographic signing, API authentication, network
submission, order matching, fills, strategies, automatic mode discovery, and
production signer deployment.
