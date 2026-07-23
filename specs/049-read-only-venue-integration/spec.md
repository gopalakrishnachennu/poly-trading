# Phase 3.2 Specification: Live Read-Only Venue Integration

## Objective

Compose public market, authenticated-user observation, REST metadata and
reference-price channels into one deterministic read-only venue supervisor with
dynamic market parameters, exchange modes, independent health, reconnect and
reconciliation gates. Order submission and cancellation are structurally absent.

## Scope

- Exact current Phase 3.1 local security report binding.
- Credentialless authenticated-observation contract with subscription-only
  event classes and no mutation endpoint.
- Independent public-market, authenticated-user, metadata and reference channel
  epochs, sequence, freshness, snapshot and provenance.
- Versioned tick size, minimum order size, maker/taker fee, taker delay and
  minimum order-age parameters using fixed-point integers.
- Normal, restarting, post-only, cancel-only, trading-disabled, recovering and
  unknown exchange modes.
- HTTP 425/restart, post-only window, cancel-only, rate-limit and independent
  channel-failure handling.
- Reconnect recovery requiring fresh snapshots for every channel, current
  parameters and exact no-mutation reconciliation.
- Journal replay, checkpoints and create-new local-certification reports.

## Exclusions

- Credential or authorization-header values
- Actual authenticated sessions before Phase 3.6
- Order, cancellation, signing, wallet, RPC or relayer endpoints
- Automatic retry, cached-book reuse across epochs or inferred exchange mode
- Live-environment certification without eligible-network evidence

## Acceptance criteria

- Stale, substituted, incomplete, real-provider-certified or authority-bearing
  Phase 3.1 evidence fails registration.
- The authenticated observation contract contains no authentication material
  and exposes only allowlisted observation event classes.
- Each channel has independent epoch, contiguous sequence, snapshot digest,
  event/receive time and health; one healthy channel cannot hide another failure.
- Dynamic parameters are version-contiguous, fixed-point, market/token-bound and
  must be revalidated after reconnect.
- Restart and channel failure invalidate readiness and create explicit recovery
  debt. Cached observations never cross the recovery boundary.
- Recovery installs a complete fresh same-epoch channel set, newer parameters,
  explicit mode and no-mutation reconciliation; it never authorizes exposure.
- Read-only readiness requires all channels fresh, current parameters, explicit
  safe observation mode and no recovery debt.
- Completion covers public/user sync, parameters, normal, restart, post-only,
  cancel-only, rate-limit, independent failure and reconnect recovery.
- Reports distinguish local certification from live environment evidence and
  grant zero authentication, signing, deployment, trading or submission authority.
- Tests and TLA+ cover channel independence, epoch replacement, modes,
  parameters, recovery, no-mutation/no-authority and absorbing halt.
