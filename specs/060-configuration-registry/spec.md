# Phase 4.4 — Configuration Registry and Terminal Settings View

## Objective

Replace terminal runtime assumptions with one typed, immutable configuration
frame that is validated before startup and visible to operators without giving
the browser any configuration-mutation authority.

## Acceptance criteria

1. Public endpoints, polling budgets, discovery windows, projection freshness,
   and client display budgets are loaded from a versioned JSON document.
2. The exact canonical configuration ID and BLAKE3 digest are exposed by a
   read-only API and displayed in the terminal.
3. Missing, malformed, expired, or unsafe configuration is explicit and forces
   `NO_TRADE`; it must not silently become a mutable runtime update.
4. Legacy compiled defaults preserve read-only observation of an already-running
   campaign, but block every new paper campaign until a current configuration
   frame is supplied and the gateway is restarted.
5. The browser has no configuration write endpoint. The settings view is
   read-only and declares that activation requires restart.
