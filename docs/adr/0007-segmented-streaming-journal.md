# ADR-0007: Segmented streaming journal and deterministic checkpoints

- Status: Accepted
- Date: 2026-07-17

## Context

The original scanner materializes every event in one file. That is correct for
small validation captures but gives memory usage proportional to journal age and
allows one file to grow without an operational boundary.

## Decision

- Add a streaming reader that allocates at most one bounded record at a time.
- Preserve the existing collecting scanner as a compatibility wrapper over the
  streaming reader.
- Rotate append-only segments before configured byte or record limits.
- Name segments by zero-padded contiguous index and reject gaps, unexpected
  files, corrupt non-final segments, and sequence discontinuity.
- Never overwrite or silently repair a segment.
- Encode replay checkpoints with an explicit versioned layout and BLAKE3
  checksum, independent of Rust memory layout.
- A checkpoint is an optimization only. Journal events remain the recovery
  authority, and checkpoint sequence/digest must match its durable prefix.

## Consequences

Replay memory becomes bounded by state plus one event rather than total event
history. Segment retention, archival, and checkpoint policy can be added without
a broker. More filesystem objects and strict directory validation are required.
