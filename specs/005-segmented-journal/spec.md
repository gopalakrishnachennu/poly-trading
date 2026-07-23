# Specification 005: Segmented Journal and Streaming Replay

## Purpose

Bound recorder file size and replay memory while preserving deterministic,
fail-closed recovery.

## Requirements

### SJR-001 Streaming reader

Read and validate one record at a time. Memory use must not grow with historical
event count. Checksum, schema, length, and incomplete-tail behavior remains
identical to the original scanner.

### SJR-002 Compatibility

`scan_path` remains available and is implemented by collecting the streaming
reader. Existing journal bytes and recovery tests remain valid.

### SJR-003 Deterministic rotation

Segments are named `segment-{index:020}.journal`. Rotation occurs before an
append that would exceed either configured record count or byte bound, except a
single valid maximum-size event may occupy its own segment.

### SJR-004 Directory integrity

Segment indices are contiguous from zero. Symlinks, unexpected entries, empty
sets during replay, corrupt segments, and incomplete tails are rejected. Only
the writer may create the next exact segment using create-new semantics.

### SJR-005 Cross-segment ordering

Recorder sequence is contiguous across segment boundaries. Duplicate,
regressing, or gapped sequence halts segmented replay.

### SJR-006 Checkpoints

Replay checkpoints explicitly encode complete `ReplayState`, schema version,
length, and BLAKE3 checksum. Decoding is bounded and rejects truncation,
trailing bytes, invalid financial values, and non-canonical books.

### SJR-007 Authority

A checkpoint never replaces journal authority. Its digest and last sequence
must equal the replayed durable prefix before later events can be applied.

### SJR-008 Scope

No database, broker, cloud storage, credential, strategy, wallet, or order
capability is introduced.

## Acceptance criteria

- Streaming and collecting readers return identical events and tail status.
- Large fixture replay demonstrates constant reader buffering.
- Rotation tests cover byte, record, restart, and oversized-single-record cases.
- Directory gap, symlink, tail, corruption, and cross-segment sequence tests fail.
- Checkpoint round-trip preserves state digest and rejects mutated bytes.
- Segmented replay produces the same digest as single-file replay.
- Formatting, Clippy, all Rust tests, and TLC pass.
