# ADR 0084: Closed official Gamma evidence is the resolution authority

- Status: Accepted
- Date: 2026-07-26

## Decision

A pinned supplemental `resolution = -1` means no supplemental label is
available. It may be backfilled only from a closed official Gamma market after
exact identity, rules, time and token verification. Existing supplemental
binary labels remain cross-checks and conflicting labels reject the batch.

## Consequences

Older independent hourly sessions can be added without trusting stale
third-party resolution metadata. Official evidence volume and request bounds
remain explicit, and unresolved or non-binary official markets stay excluded.
