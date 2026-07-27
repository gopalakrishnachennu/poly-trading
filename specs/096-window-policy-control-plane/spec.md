# Specification 096: Window Policy Control Plane (Paper Vertical Slice)

## Objective

Make operational parameters for window paper setups loadable from versioned,
digest-bound configuration so operators can change thresholds without editing
Rust — starting with a single vertical slice: `5m_late_continuation`.

## Problem

Entry windows, confidence bands, and ask caps for setups live as compile-time
constants in `setups.rs` / `config.rs`. Editing them requires a code change and
redeploy. The control-plane redesign requires a proven path: one setup, defaults
identical to today's behavior, journaled configuration identity, and a minimal
activate API — before migrating the remaining seven setups.

## Scope

- Typed parameter schema for `5m_late_continuation` (JSON Schema metadata for GUI).
- Compiled defaults equal to current hardcoded values (230–245s, confidence
  600k–820k, max ask 820k, require positive medium momentum).
- Versioned policy files under `var/strategy-control/` with blake3 digest.
- `active.json` pointer to the deployed version; missing pointer → in-memory
  defaults (behavior unchanged).
- Corrupt active pointer or unreadable version file → halt recovery/start (do
  not silently skip).
- Journal `configuration_digest` on decisions (optional for pre-096 journals).
- Open positions store `configuration_digest` at open; exits keep that snapshot.
- Minimal HTTP API: list plugin, read schema, save draft version, validate,
  activate, read active/effective config.
- Paper-only; no live trading authority.

## Exclusions

- Experiments, live lifecycle, float metrics.
- Auto-apply from Advisor MCP.
- Hot-reload of parameters on already-open positions.

## Phase status

- Phase 2 (this slice): done — `5m_late_continuation` policy-bound.
- Phase 3: done — schema-driven Strategy Builder (`STRATEGIES` / F9).
- Phase 4: done — remaining seven setups share `SetupGateParams`; compiled
  defaults ≡ pre-policy hardcodes; GUI edits any of the eight plugins.

## Phase 4 acceptance criteria

9. Compiled defaults for all eight setups match the pre-policy hardcodes
   (regression tests).
10. Disabling any setup via policy rejects that setup id without granting live
    authority.
11. Phase-2-only version files deserialize with defaulted sibling setups and
    validate.
12. Strategy Builder / API expose all eight plugin schemas and save a full bag.

## Acceptance criteria

1. With no `var/strategy-control/active.json`, engine behavior for
   `5m_late_continuation` matches pre-096 hardcoded gates (regression tests).
2. Activating a valid version with a wider entry window causes entries outside
   230–245 when those params are set (proof config is live).
3. Every new open decision journals `configuration_digest` matching the active
   (or default) policy digest.
4. Legacy journals without `configuration_digest` still deserialize.
5. Invalid policy (entry_hi < entry_lo, live_trading true, non-paper) cannot
   activate; validator returns clear errors.
6. Corrupt active version file refuses start / sets halt — never silent skip.
7. No floating-point types in policy money/threshold fields.
8. API can return schema, defaults, active digest, and accept activate of a
   validated version.
