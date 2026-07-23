# Phase 4.3 — Paper Market Policy Frame

## Objective

Remove embedded paper-trading economics and bind every new paper campaign to
one immutable, validated policy document.

## Acceptance criteria

1. A new paper campaign is rejected unless a policy file is loaded, current,
   digest-valid, and explicitly permits every requested asset.
2. Fee, slippage, minimum locked edge, and maximum pair quantity are read from
   the bound per-asset policy; no execution economics are embedded in code.
3. The policy ID and digest are journaled at campaign start and returned by the
   status API for terminal display.
4. The terminal exposes policy status but cannot edit or authorize a policy.
5. Existing unbound campaigns recover only in conservative observation mode;
   they cannot open a simulated pair after recovery.
