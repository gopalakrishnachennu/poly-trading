# ADR 0062: Bind paper economics to an immutable policy frame

## Status

Accepted for Phase 4.3.

## Context

Paper campaigns previously embedded a fee reserve, slippage reserve, minimum
edge, pair-size cap, and a small asset list in application code. That hides
operator assumptions, makes comparative campaigns ambiguous, and risks a
restarted campaign applying economics different from those used at creation.

## Decision

Every new paper campaign requires an operator-supplied JSON policy with a
bounded schema, validity window, allowed asset set, and fixed-point per-asset
fee, slippage, minimum locked edge, and maximum pair quantity. The gateway
canonicalizes and BLAKE3-digests the validated policy, journals its ID and
digest at campaign start, and snapshots it in the in-memory campaign state.

Recovery may restore a campaign's simulated-pair capability only when the
currently supplied policy is valid and has the exact journaled ID and digest.
Missing, legacy, expired, invalid, or mismatched policies leave the campaign
running only as an observation recorder and force `NO_TRADE` decisions. The
browser can display this status but cannot alter policy or grant authority.

## Consequences

- Paper economics are explicit, auditable, and comparable across campaigns.
- The active asset universe is configured for paper decisions, not embedded in
  the strategy loop.
- No policy file grants signing, wallet, authenticated transport, order, or
  live-trading authority.
