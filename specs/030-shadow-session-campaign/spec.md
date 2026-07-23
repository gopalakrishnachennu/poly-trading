# Phase 2.17 Specification: Deterministic Shadow-Session Campaign Runner

## Objective

Replay bounded multi-day recorded-session campaigns through Phase 2.16,
exercise certification renewal/expiry, partition/dead-man, restart and unknown-
state recovery schedules, and produce a checksummed non-authorizing operator
evidence bundle.

## Scope

- One single writer owns one Phase 2.16 gateway for the complete campaign.
- An immutable manifest binds campaign identity, exact session windows,
  recording digests, required scenarios, limits, and final schedule-chain digest.
- Steps are globally contiguous, timestamp-monotonic, hash-chained, bounded,
  and applied exactly once.
- Runtime replay steps require the exact active recorded session. Control steps
  may renew certification, update heartbeat, inject recorded fixtures, tick
  expiry, or provide explicit recovery evidence between sessions.
- Coverage is derived from authentic Phase 2.16 outcomes, never caller claims.
- Final evidence records missing coverage, incomplete sessions, schedule
  mismatch, halted/degraded final state, and unresolved backing independently.
- An eligible evidence bundle still requires an explicit future operator
  decision and grants no promotion, deployment, or live authority.
- Campaign commands and complete nested state are journaled and checkpointed.

## Exclusions

- Credentials, keys, signatures, authenticated transport, RPC, wallets
- Relayer clients, live orders, transactions, deployment or automatic retry
- Live venue input or production capital
- Automatic promotion from an evidence result

## Acceptance criteria

- A campaign shorter than the configured multi-day duration, with too few
  sessions, overlapping sessions, duplicate identities, or invalid bounds is
  rejected before state installation.
- Step gaps, sequence, time, session ownership, previous digest, and step digest
  are exact and fail closed.
- All required certification, partition, dead-man, restart, unknown-state and
  recovery coverage is derived from accepted gateway outcomes.
- Finalization is `PROMOTION_ELIGIBLE` only after the exact full schedule,
  every session is closed, every required scenario is covered, Phase 2.16 is
  ready, and no reservation or conversion remains unresolved.
- Evidence files are create-new, bounded, versioned, canonical, checksummed,
  and revalidate their internal bundle digest.
- Replay and checkpoint recovery reproduce the campaign and nested gateway
  digest exactly; durable corruption or sync failure is fail-closed.
- Tests include multi-day success, missing coverage, certification expiry,
  partitions, dead-man, restart/unknown recovery, schedule substitution,
  durability failure, and deterministic replay.
- A bounded TLA+ model proves ordered sessions, coverage-gated eligibility,
  unresolved-backing denial, no automatic promotion/live authority, and halt
  absorption.
