# ADR 0099: Window Policy Is Digest-Bound And Paper-Only

## Status

Accepted

## Context

Operators need to change setup thresholds without source edits, but the window
engine must stay deterministic, replayable, and proposal-only. Silent env
mutation or mutable “current config” would break journal identity and recovery.

## Decision

- Introduce a **Window Policy Control Plane** for paper: versioned JSON policy
  documents under `var/strategy-control/`, each with a blake3 digest.
- Runtime loads only the **active** digest (or compiled defaults if none).
- Decisions and opens record `configuration_digest`.
- Open positions keep the digest from entry; parameter changes apply to new
  entries only.
- Phase 2 migrated one setup (`5m_late_continuation`) with defaults equal to
  pre-policy constants.
- Phase 4 migrates the remaining seven setups onto the same `SetupGateParams`
  bag; compiled defaults remain byte-identical to the former hardcodes. Older
  Phase-2 version files deserialize with defaulted siblings and may rewrite the
  active digest on load.
- Live trading flags in policy are rejected. The GUI/API cannot grant live
  authority.
- Corrupt active policy fails closed (halt), matching journal recovery rules.

## Consequences

All eight paper setups are operator-editable under one digest-bound document.
Threshold changes apply to new entries only; open positions keep the entry
digest. Schema-driven Strategy Builder (Phase 3/4) edits one plugin at a time
and merges into the full bag before save/activate. New alpha still requires a
Rust plugin.
