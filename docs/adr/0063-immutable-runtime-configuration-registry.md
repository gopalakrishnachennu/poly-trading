# ADR 0063: Use immutable runtime configuration frames

## Status

Accepted for Phase 4.4.

## Decision

The terminal loads one typed JSON configuration frame at gateway startup. It
contains public sources, polling and discovery budgets, projection freshness
budgets, response bounds, and browser display budgets. The document is
canonicalized and BLAKE3-digested; its ID and digest are exposed through a
read-only endpoint and bound to every new paper-campaign journal record.

The browser may display effective values but has no configuration write API.
The gateway never watches or hot-reloads configuration. Any change requires a
new validated startup, making the previous and next configuration identities
auditable. Invalid or missing configuration uses only compiled safe defaults
for legacy public observation, remains explicitly `NO_TRADE`, and blocks every
new paper campaign.

## Consequences

- Financial and operational assumptions become inspectable rather than hidden.
- An active legacy recorder is not interrupted merely because it predates this
  control, but it cannot gain simulated-trading capability.
- Fixed-point units, schema compatibility, memory ceilings, durable integrity,
  and fail-closed rules remain code invariants, not editable settings.
