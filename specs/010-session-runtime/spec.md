# Phase 1.9 Specification: Durable Read-Only Session Runtime

## Objective

Make hourly coordination recoverable and operational by recording every
registration and coordination frame before applying it, validating durable
prefix checkpoints, and owning live mutations in one bounded actor.

## Requirements

1. Use the existing checksummed, append-only segmented journal and contiguous
   sequence enforcement.
2. Encode registrations and complete coordination frames with an explicit,
   versioned, bounded schema using integer fixed-point values only.
3. Append and device-sync every command before applying it to coordinator
   state. A journal or sync failure must never mutate live state.
4. Replay uses the same coordinator transitions and rejects wrong source,
   identity, timestamps, malformed payloads, gaps, corruption, and tails.
5. A checkpoint attests an exact journal sequence and coordinator digest. It
   must be validated against the replayed durable prefix before later records
   are accepted; it is never an independent authority.
6. The Tokio wrapper has one writer, bounded ingress, immutable watch
   snapshots, explicit full/closed failures, and terminal halt behavior.
7. Runtime restart recovers the durable journal before opening the writer and
   resumes at exactly the next sequence.
8. Shutdown synchronizes the journal and returns a terminal snapshot.
9. No network, discovery, strategy, authenticated channel, signing, or order
   capability exists in this crate.

## Acceptance criteria

- Canonical codec round trips and rejects versions, bounds, invalid enums,
  invalid fixed-point values, trailing data, and malformed JSON.
- Tests prove journal-before-apply, no mutation on durable failure, restart
  equivalence, checkpoint validation, corruption rejection, bounded ingress,
  terminal halt, and clean shutdown.
- Online, journal replay, checkpoint-validated replay, and restarted runtime
  produce identical coordinator digests.
- Formatting, Clippy with denied warnings, all workspace tests, and TLC pass.

## Exclusions

External network orchestration, predictive models, strategies, credentials,
authenticated exchange state, orders, positions, capital allocation, signing,
and databases.
