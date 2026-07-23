# ADR-0004: Public market-data boundary

- Status: Accepted
- Date: 2026-07-16

## Context

Hourly market identity and order-book events arrive through independent public
Gamma REST and CLOB WebSocket interfaces. Their payloads are untrusted, evolve
independently, and contain encoded JSON strings for outcomes and token IDs.

## Decision

Create a read-only `public-market-data` boundary that:

- discovers only configured hourly series using keyset pagination;
- validates immutable condition, question, token, outcome, time, and rules data;
- fingerprints resolution rules;
- subscribes to the public market socket by validated token IDs;
- bounds every REST page and WebSocket message before parsing;
- journals normalized per-market events with event and receive timestamps;
- reconnects by rediscovering markets and resubscribing for fresh snapshots.

The crate contains no authenticated endpoint, wallet, signing, order, or
position capability.

## Consequences

Unknown or malformed market metadata is rejected rather than guessed. A socket
reconnect starts a new synchronization epoch; downstream trading will remain
disabled until fresh book snapshots are observed for every subscribed asset.

