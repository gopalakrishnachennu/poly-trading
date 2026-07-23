# ADR-0002: Explicit checksummed event journal

- Status: Accepted
- Date: 2026-07-16

## Context

Recovery and replay require a durable record that does not depend on a database,
broker, compiler memory layout, or a serialization library's default settings.

## Decision

Use an explicit little-endian event envelope stored in append-only journal
records. Each record carries a length and checksum. Incomplete tails may be
truncated during recovery; checksum or structural corruption halts recovery.

## Consequences

Schema changes require versioning and compatibility tests. The journal is usable
before PostgreSQL, ClickHouse, or Redpanda exists.

