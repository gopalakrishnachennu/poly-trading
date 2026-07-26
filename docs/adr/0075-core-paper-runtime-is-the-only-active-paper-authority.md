# ADR 0075: The core-backed Paper Week runtime is the only active paper authority

- Status: Accepted
- Date: 2026-07-26

## Decision

The managed stack will not launch either legacy Python bankroll simulator. Both
use separate floating-point accounting and bypass the core paper runtime's
fixed-point policy, ledger, reservations, scenario risk, lifecycle and
reconciliation gates. They remain historical research evidence and require two
explicit deployment-only acknowledgements for any isolated replay experiment.

The terminal treats Paper Week as the sole active paper-execution interface.

## Consequences

No legacy report can be mistaken for the account used by the core paper
campaign. Retiring this path does not alter its data. It creates no model,
strategy approval, wallet, credential, signer, order, or live authority.
