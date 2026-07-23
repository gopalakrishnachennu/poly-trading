# ADR-0001: Rust deterministic kernel

- Status: Accepted
- Date: 2026-07-16

## Context

The live platform will control financial state under tight latency and recovery
requirements. Runtime pauses, implicit numeric conversions, and ambiguous state
ownership increase operational risk.

## Decision

Use Rust for the trading kernel and authoritative live state machines. Use one
single-writer actor per market and checked fixed-point financial primitives.
Python is permitted for offline research but not for authoritative order,
position, signing, reconciliation, or capital-floor control.

## Consequences

Rust toolchains and CI are mandatory. Unsafe code is forbidden by default.
Concurrency is introduced across actors rather than inside owned market state.

