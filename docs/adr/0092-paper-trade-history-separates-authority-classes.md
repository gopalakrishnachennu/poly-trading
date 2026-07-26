# ADR 0092: Paper Trade History Separates Authority Classes

## Status

Accepted

## Context

The repository contains fills from the core fixed-point paper campaign and
historical trades from two retired Python simulators. A single trade export is
useful for inspection, but combining these sources without explicit authority
and provenance fields would falsely imply that every row came from the current
paper engine or from one shared bankroll.

## Decision

- Core paper fills are exported only after complete journal digest, sequence,
  campaign, and decision/fill verification.
- Legacy simulator decimal values remain source-preserved decimal text.
- The retired intraday journal omitted its strategy name, so the export may
  infer that name only from the checksum-bound retired engine contract and must
  label the provenance as inferred.
- Every record carries an authority class, engine status, execution mode,
  strategy provenance, and `real_money=false`.
- The export does not aggregate independent simulator bankrolls into one
  portfolio return.

## Consequences

Operators receive one inspectable CSV without confusing historical simulations
with active core paper authority. There are zero real-money trades in the
artifact, and the artifact grants no trading or model-promotion authority.
