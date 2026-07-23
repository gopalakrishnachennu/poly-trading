# Agent Instructions

## Mission

Build a deterministic, event-driven prediction-market trading platform with
independent risk, reconciliation, settlement, and audit controls.

## Required reading order

1. `docs/PROJECT_CHARTER.md`
2. `docs/FINANCIAL_INVARIANTS.md`
3. `docs/SYSTEM_ARCHITECTURE.md`
4. `docs/CURRENT_STATE.md`
5. The assigned specification under `specs/`
6. Relevant accepted ADRs under `docs/adr/`

## Non-negotiable rules

- Never represent money, price, quantity, fees, or capital floors with binary
  floating point.
- Strategy code may never sign or submit an order directly.
- Matched inventory is not confirmed or spendable inventory.
- New exposure is forbidden whenever authoritative state is stale, unknown, or
  unreconciled.
- Every external event must preserve event time and local receive time.
- Every state transition needed for recovery must be journaled.
- Corrupt durable state must halt recovery; it must never be silently skipped.
- No live trading until recorder, replay, ledger, reconciliation, risk, paper,
  and shadow-production gates have passed.
- Production secrets, keys, wallet material, logs, and database dumps must not
  enter Git or AI context.
- Unsafe Rust is forbidden unless an accepted ADR isolates and justifies it.

## Change workflow

1. Work from a specification with explicit acceptance criteria.
2. Keep scope within the current milestone.
3. Add or update tests with behavior changes.
4. Record architecture changes as ADRs; never silently reverse an accepted ADR.
5. Run formatting, linting, and all workspace tests.
6. Update `docs/CURRENT_STATE.md` only when milestone status materially changes.

## Definition of done

- Acceptance criteria are demonstrably satisfied.
- Financial and recovery invariants remain true.
- Tests include failure behavior, not only happy paths.
- `cargo fmt`, Clippy with warnings denied, and all tests pass.
- Documentation and state-machine terminology remain consistent.
- No unrelated infrastructure or abstraction was added.

