# Phase 3.7 Specification: Micro-Capital Canary Controls

## Objective

Implement deterministic canary eligibility controls for allowlisted complete-set
opportunities under tiny fixed capital, capital-floor, session-loss and exposure
ceilings, without granting capital or live execution authority.

## Scope

- Exact current Phase 3.6 local report binding.
- Exact market/condition/Up/Down allowlist.
- Complete-set-only candidate and explicit `NO_TRADE` support.
- Signed fixed-point capital, floor, session-loss and exposure limits.
- Distinct risk and operations approval subjects.
- Kill switch, dead-man cancellation, operator abort and rollback simulations.
- Journal replay, checkpoints and create-new reports.

## Exclusions

- Real capital, wallet, signing, transport or live order submission
- Directional or sequential-hedge canary strategies
- Guaranteed trade, principal preservation or return
- Live canary completion without external authorization and evidence

## Acceptance criteria

- Registration rejects stale, authority-bearing Phase 3.6 evidence and invalid,
  nonpositive or internally inconsistent fixed-point limits.
- Exact allowlist identity and complementary tokens are non-substitutable.
- Two distinct opaque operators approve one exact plan; labels are not
  credentials or signatures.
- Complete-set candidate cost, worst-case wealth, session loss and exposure are
  checked independently using signed integers and checked arithmetic.
- Capital-floor, loss, exposure and allowlist violations deterministically
  produce denial and no placement authority.
- Kill switch is irreversible; dead-man, operator abort and severe health cases
  require simulated cancellation/rollback while retaining ambiguous backing.
- `NO_TRADE` is a successful safe outcome and does not reserve capital.
- Final report is code-eligible only, live canary complete is false and every
  credential, capital, signing, deployment, trading and submission flag is false.
- Tests and TLA+ cover ceilings, allowlist, dual control, kill/dead-man,
  rollback, no authority and absorbing halt.
