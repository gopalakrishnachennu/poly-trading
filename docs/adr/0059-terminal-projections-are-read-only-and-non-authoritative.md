# ADR 0059: Terminal projections are read-only and non-authoritative

## Status

Accepted

## Context

An operator terminal needs current public market state, but a browser must not
become a financial state owner or silently manufacture values when upstream
data is unavailable. Projecting trading-kernel state is distinct from granting
strategy, risk, accounting or execution authority.

## Decision

A Rust-owned gateway validates exact hourly market identities, complementary
public books and reference observations before publishing a versioned read-only
projection. Financial values cross the HTTP boundary as decimal integer strings.
Each refresh is all-or-nothing per asset; rollover and failures clear readiness.
Stale or unavailable state produces an attributable `NO_TRADE` projection.

The browser may retain received samples for visualization but cannot originate
financial truth. Capital, reservations, inventory, P&L, reconciliation and
execution state remain unavailable until their authentic Rust projections are
connected.

## Consequences

- The terminal cannot confuse demo values with live observations.
- A single failed complementary leg invalidates the complete asset projection.
- Public data visibility adds no credential or order capability.
- Later authenticated projections require a separate reviewed boundary.
