# ADR 0061: Freeze Paper Datasets Before Model Evaluation

- Status: Accepted
- Date: 2026-07-22

## Decision

Paper journals are converted into bounded immutable manifests before research
models are trained or evaluated. The conversion verifies every raw record
digest, campaign identity, sequence and timestamp. Strategy-available time is
explicitly preserved as the local recorded time; it is never replaced by a
later-corrected value.

Train, validation and test folds are chronological, disjoint and grouped at
equal availability timestamps. A submitted artifact must bind the exact train,
validation, feature, configuration and dataset digests. Dataset submission is
not promotion and grants no financial or execution authority.

## Consequences

The learning workflow can use the one-week paper campaign without silently
leaking final outcomes or corrected observations into training. Incomplete
campaigns, corrupted journals and insufficient distinct time buckets produce no
dataset and therefore no model evaluation.
