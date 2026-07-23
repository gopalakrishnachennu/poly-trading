# Phase 2.16 Specification: Credentialless Shadow Gateway Harness

## Objective

Compose a fresh Phase 2.15 certification report with the Phase 2.14 unified
offline runtime, translate recorded adapter fixtures into deterministic
simulated gateway observations, and fail closed on heartbeat, restart,
reconciliation, or certification-freshness failures.

## Scope

- One single writer owns the gateway state and the nested Phase 2.14 runtime.
- A certified report is evidence only and never grants live authority.
- New shadow exposure requires a fresh matching certification, a completely
  healthy stack heartbeat, and a placement-capable simulated gateway mode.
- Fixture translations may create only deterministic Phase 2.14 mode
  observations or inert gateway-control observations.
- Restart recovery requires fresh certification, healthy heartbeat, current
  nested reconciliation, and digest-bound explicit clearance of unknown orders.
- Heartbeat loss activates the simulated dead-man switch, disables new
  exposure, and never releases existing backing.
- Commands are canonical, bounded, journal-first, replayable, and checkpointed.

## Exclusions

- Credentials, private keys, signatures, authenticated APIs, RPC and wallets
- HTTP, WebSocket, relayer, order or transaction submission
- Automatic retry or deployment authority
- Live exchange data and live capital

## Acceptance criteria

- Expired, missing, mismatched, non-certified, or authority-granting reports
  cannot enable new shadow exposure.
- Any unhealthy stack component or expired heartbeat triggers the simulated
  dead-man switch and a derived `TRADING_DISABLED` Phase 2.14 mode.
- Restart, rate-limit, unknown-order, settlement-retry, and heartbeat fixtures
  retain backing and prohibit unsafe automatic progress.
- Caller-supplied Phase 2.14 mode observations are rejected; mode provenance is
  derived only by the harness.
- Exact duplicate commands are no-ops; identity/history equivocation halts.
- Replay and checkpoint recovery reproduce the complete nested digest.
- Tests cover expiry boundaries, restart recovery, fixture translation,
  unhealthy heartbeats, command gating, durability failure, and soak behavior.
- A bounded TLA+ model proves certification/heartbeat gating, dead-man safety,
  restart recovery ordering, no live authority, and halt absorption.
