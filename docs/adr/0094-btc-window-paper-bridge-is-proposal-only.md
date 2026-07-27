# ADR 0094: Managed BTC Window Paper Bridge Is Proposal-Only

## Status

Accepted

## Context

Spec 089 delivered the every-second BTC strategy engine as a pure evaluation
crate. Operators still need a managed process that consumes live 5m/15m/1h
observation feeds and journals paper decisions. That process must not revive
retired Python simulators or bypass the core Paper Week capital/risk boundary.

## Decision

- Add a supervised `btc-window-paper` binary in `btc-window-strategy` that polls
  the unified windows feed, evaluates `btc_window_engine_v1`, and appends
  fixed-point decision journals under `var/btc-window-paper/`.
- Treat the worker as paper-observation / proposal evidence only. It must not
  reserve Paper Week capital, approve risk, sign, or submit orders.
- Keep complete-set Paper Week as the separate pair-execution paper authority.
- Keep legacy directional and fixed-threshold Python workers DISABLED.

## Consequences

The managed stack can collect ACTIVE_CORE and SHADOW_EXPERIMENT evidence for
BTC multi-window strategies with auditable journals. Real-money deployment and
Paper Week capital mutation remain out of scope until later gated specs.
