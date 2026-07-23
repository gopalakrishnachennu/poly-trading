# Phase 4.2 Specification: Frozen Paper-Learning Dataset

## Objective

Convert a completed paper campaign journal into an immutable, replay-verifiable
dataset with chronological train/validation/test folds for offline research.

## Scope

- Bounded JSONL paper-journal decoding and BLAKE3 record-digest verification.
- Sequence, campaign identity, timestamp and duplicate-record validation.
- Explicit source event, receive and strategy-available timestamps.
- Disjoint chronological folds that never split the same availability-time
  bucket across train, validation and test data.
- Immutable dataset manifest and model-artifact submission binding.

## Exclusions

- Training, online learning, parameter mutation, model promotion, credentials,
  signing, capital, risk approval, placement and submission.

## Acceptance criteria

- Corrupt, oversized, out-of-order, duplicate, mixed-campaign, future or
  timestamp-invalid journal records fail dataset construction.
- Test data cannot appear in a model artifact's training or validation binding.
- Every fold is non-empty, disjoint and chronological by strategy-available
  time; equal-time records remain in one fold.
- Dataset and submission digests reproduce exactly from the journal prefix.
- Outputs remain paper-only and grant no financial or execution authority.
