# Specification 001: Deterministic Market Recorder

## Purpose

Capture canonical external events in an append-only format that supports exact
decoding, replay, audit, and safe crash-tail recovery.

## Requirements

### MR-001 Canonical envelope

An envelope contains schema version, source, source sequence, event time, receive
time, market identifier, and opaque normalized payload.

### MR-002 Deterministic encoding

The same envelope must encode to identical bytes across repeated executions on
the same schema version. Encoding must not depend on Rust memory layout.

### MR-003 Bounded inputs

Market identifier and payload lengths are bounded before allocation.

### MR-004 Durable records

Every journal record includes a byte length and checksum. Successful append
flushes user-space buffers; explicit synchronization is available to callers at
durability boundaries.

### MR-005 Recovery

Scanning returns every valid record before an incomplete tail. Recovery may
truncate only an incomplete tail. Checksum, magic, or structural corruption is a
hard error and must not be skipped.

### MR-006 No trading

The recorder contains no credential, signing, order, or strategy capability.

## Acceptance criteria

- Encoding round trips and is byte-for-byte deterministic.
- Oversized and malformed envelopes are rejected before large allocation.
- Multiple records scan in original order.
- A partial header and partial body are recognized as truncated tails.
- Recovery truncates a partial tail to the last valid boundary.
- A checksum mismatch produces a corruption error and leaves the file unchanged.
- Financial primitives use no floating point and pass property tests.

