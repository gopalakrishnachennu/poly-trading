# Phase 4.9 Specification: Segmented Raw Tick Capture

## Objective

Replace unbounded single-file public tick capture with explicit, checksummed,
bounded segment directories.  Completed segments are immutable export inputs;
the active segment remains capture-only until a later explicit snapshot/export
operation verifies a complete prefix.

## Scope

- Add an explicit `--segments <directory>` mode to the public CLOB and
  reference-market recorder binaries.
- Preserve the existing single-file command as an explicit backwards-compatible
  mode; never silently migrate, truncate, rename, or delete old journals.
- Make the managed capture launcher select segment directories for its next
  explicit start, with a bounded 256 MiB / 1,000,000 record segment policy.
- Document the layout and the fact that an operator must explicitly stop the
  existing legacy recorder before starting the segmented recorder.

## Exclusions

- Exporting an active segment, changing capture processes already running,
  retention/deletion, strategy changes, model training, paper execution,
  wallets, credentials, signing, authenticated transport, and live orders.

## Acceptance criteria

- Segment mode uses the existing `SegmentedJournalWriter`, preserving
  contiguous event sequence across every segment.
- An invalid/symlinked/gapped/incomplete existing segment directory fails
  closed before capture begins.
- The launcher creates no migration or deletion side effect and announces the
  exact selected capture layout.
- Single-file capture remains supported only when explicitly requested.
- Formatting, warnings-denied Clippy, and affected tests pass.
