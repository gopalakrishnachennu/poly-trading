# ADR 0093: BTC Multi-Window Every-Second Strategy Engine

## Status

Accepted

## Context

Historical paper evidence used several independent Python directional and
intraday simulators plus a small complete-set core journal. Those directional
rules were retired for unsafe floating-point bankrolls, poor payoff asymmetry,
or negative out-of-sample P&L. A new BTC 5m/15m/1h specification requires one
combined every-second decision engine with fixed-point economics, universal
exits/hedges/reversals, and separated ACTIVE_CORE versus SHADOW_EXPERIMENT
books.

## Decision

- Introduce the proposal-only `btc-window-strategy` crate as the sole active
  BTC multi-window directional/hedge evaluation boundary for paper mode.
- Evaluate markets once per second: validate feeds, build features, manage any
  open position before ranking a single new candidate, then journal the
  decision and every rejection reason.
- Keep 5m and 15m setups as CORE CANDIDATE paper paths; keep 1h directional
  and sequential-hedge entries as SHADOW/EXPERIMENTAL until volume gates pass.
- Treat complete-set and sequential locks as execution-dependent scanners that
  must not force trades and must protect orphan legs.
- Retain legacy strategies only as DISABLED features or RETIRED_LEGACY evidence;
  they must not place orders.
- Preserve ADR 0022 / 0051: strategies propose and never authorize, sign, or
  submit.

## Consequences

Paper operators gain one auditable BTC window engine with shared risk and
exit logic. Real-money deployment remains blocked until the specification's
validation gates are met. Existing complete-set core paper authority is not
auto-replaced; this crate adds the multi-window directional/hedge surface for
paper evidence collection under separate performance books.
