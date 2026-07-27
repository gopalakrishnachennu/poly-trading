# Specification 095: Last 48-Hour Capture and Trade Export

## Objective

Publish one deterministic, operator-readable export bundle covering the exact
last 48 hours of locally captured BTC and ETH 5-minute, 15-minute, and one-hour
market books, reference observations, strategy decisions, and every recorded
paper trade lifecycle event.

## Scope

- Freeze one explicit UTC `[start, end]` interval.
- Freeze every active JSONL input at its newline-complete byte prefix before
  parsing; later appends cannot enter the export.
- Export one full-depth CSV for each asset/window pair:
  `BTC|ETH × 5m|15m|1h`.
- Export immutable market identities, hourly reference snapshots, Coinbase
  reference observations, and every current window-strategy decision including
  `NO_TRADE`.
- Export all simulated trade lifecycle events from:
  - the current fixed-point BTC-window paper journal;
  - the core Paper Week complete-pair journal;
  - retired directional and intraday simulator journals.
- Preserve source paths, line numbers, authority class, strategy attribution,
  paper/live status, fixed-point fields, and raw legacy decimal text.
- Publish source-prefix evidence, per-output SHA-256 checksums, row counts, and
  timestamp coverage in a manifest and source-manifest CSV.

## Exclusions

- Real-money trades or live order authority.
- Resampling, interpolation, fabricated seconds, or inferred fills.
- Deduplicating captured events. Exact duplicates remain rows and are marked.
- Treating local capture time as venue event time.
- Combining independent paper bankrolls into one portfolio P&L.

## Acceptance criteria

1. Every selected input record is inside the frozen interval and every output
   row carries its original source path and line.
2. A complete market-book row preserves all bid and ask levels and identifies
   the token side through an immutable market identity.
3. Conflicting identities, malformed complete JSONL records, invalid fixed-point
   values, or non-paper trade records fail the export.
4. An incomplete active-file tail is excluded and counted, never parsed or
   silently treated as complete.
5. Strategy decisions retain `NO_TRADE`; trade events retain entries, hedges,
   reductions, exits, settlements, and core pair decisions/fills.
6. Output row counts and SHA-256 values reconcile with the manifest.
7. No output claims real-money execution or model/live promotion authority.
