# Specification 003: Deterministic Order-Book Replay

## Purpose

Decode public journal events and reconstruct token order books exactly and
repeatably without network or trading capability.

## Requirements

### OBR-001 Fixed-point decoding

Prices and quantities are parsed directly from bounded decimal strings into
millionths. Binary floating point, exponent notation, signs, favorable rounding,
and excess non-zero precision are rejected.

### OBR-002 Payload integrity

The typed payload prefix version, kind, timestamp, asset list, JSON length, JSON
event type, condition ID, token IDs, and envelope metadata must agree. Truncated
or trailing bytes are hard errors.

### OBR-003 Sequence integrity

Recorder sequence must be contiguous and increasing. Duplicate, decreasing, or
gapped sequence halts replay.

### OBR-004 Epoch safety

An epoch start clears all books and marks state unsynchronized. Each token needs
a new `book` snapshot before deltas can apply. The recorded synchronized marker
is accepted only after at least one fresh snapshot. Shutdown disables updates.

### OBR-005 Book invariants

Snapshots reject duplicate levels, zero-sized levels, and crossed books. Deltas
replace a level quantity or delete it when quantity is zero. A crossed delta or
reported-best mismatch marks that token book non-authoritative and requires a
fresh snapshot; subsequent deltas cannot make it authoritative again. Bids are
ordered descending for consumption and asks ascending, while storage remains
canonical.

### OBR-006 Replay equivalence

Replaying identical envelopes produces identical state and a stable digest that
does not depend on hash-map order or Rust memory layout.

### OBR-007 Read-only boundary

The replay crate contains no HTTP, WebSocket, credential, wallet, signing,
strategy, or order-submission capability.

## Acceptance criteria

- Decimal boundary/property tests prove exact millionth conversion.
- Golden payloads decode into typed events and reject identity mismatches.
- Snapshot and delta tests cover insert, replace, delete, crossed, and stale
  books.
- Epoch and sequence failure tests halt conservatively.
- Two independent replays of the same journal have the same digest.
- Formatting, Clippy, all Rust tests, and TLC pass.
