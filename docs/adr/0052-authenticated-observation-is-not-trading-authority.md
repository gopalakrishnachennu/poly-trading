# ADR 0052: Authenticated Observation Is Not Trading Authority

- Status: Accepted
- Date: 2026-07-21

## Decision

Authenticated user-channel observation is modeled as a distinct subscription-
only capability. Its contract contains no credential value and exposes no order,
cancel, signing, wallet or relayer operation. A healthy authenticated channel is
necessary for reconciliation but never sufficient for exposure or execution.

Public, user, metadata and reference channels have independent health. Restart
or failure invalidates cached readiness. Recovery requires fresh snapshots from
all channels, current market parameters, explicit exchange mode and exact
no-mutation reconciliation.

## Consequences

- Losing the user channel cannot be hidden by a healthy public book.
- Authentication does not imply mutation capability.
- Cached books and parameters cannot silently cross reconnect epochs.
- Phase 3.6 must prove real authenticated observation preserves this boundary.
