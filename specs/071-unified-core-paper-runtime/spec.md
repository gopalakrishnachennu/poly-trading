# Phase 5.5 Specification: Unified Core-Backed Paper Runtime

## Objective

Remove legacy dashboard-local and Python bankroll simulators from the managed
runtime. The core-backed, fixed-point Paper Week campaign is the only active
paper-execution authority.

## Scope

- Retire managed startup and default direct invocation of the legacy directional
  simulator and the fixed-threshold intraday simulator.
- Preserve their reports and JSONL evidence without modification.
- Ensure terminal APIs label legacy views as retired and direct operators to the
  core Paper Week campaign.
- Make managed-stack status derive from actual process identities rather than
  stale background-shell variables.

## Exclusions

- Changing historical evidence, retraining models, approving a strategy,
  live orders, credentials, signing, wallets, or external execution.

## Acceptance criteria

- A normal managed-stack start creates no legacy simulated bet.
- A direct legacy invocation creates no entry without two exact
  deployment-only acknowledgements.
- The terminal cannot present a legacy bankroll as an active paper account.
- Historical reports remain readable, and the core Paper Week controls remain
  the sole active paper-execution interface.
