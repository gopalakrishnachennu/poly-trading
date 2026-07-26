# Phase 5.21 Specification: BTC Multi-Window Per-Second Export

## Objective

Extend the checksum-bound BTC per-second research boundary to authentic
15-minute and one-hour local captures and publish one verified CSV for each
window.

## Scope

- Accept only the explicit `5m`, `15m` and `1h` window contracts with exactly
  300, 900 and 3,600 second rows respectively.
- Reuse the same identity, depth, fixed-point, causal-availability, freshness,
  quarantine, partition and checksum rules across every window.
- Freeze newline-complete source evidence separately for each timeframe.
- Preserve missing and carried observations explicitly rather than fabricating
  ticks.
- Publish one content-addressed partitioned export and one independently
  verified single-file derivative per timeframe.

## Exclusions

- Resampling one timeframe into another, interpolation, backward fill,
  invented market identity, model labels, model training, paper authority,
  capital authority or live execution.

## Acceptance criteria

- Every accepted 15-minute condition has exactly 900 unique rows.
- Every accepted one-hour condition has exactly 3,600 unique rows.
- Source, manifest and CSV checksums reconcile before publication.
- An altered existing output cannot be silently reused.
- The 5-minute export remains deterministic and backward compatible.
