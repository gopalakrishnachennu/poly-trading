# ADR 0095: The Paper Window Book Is Marked From Observed Quotes

## Status

Accepted

## Context

Specs 089 and 090 produced a decision tape: what the engine proposed and why.
That tape cannot answer the two questions an operator actually has — how much
was committed, and did the setup make money. Without a stake and a result,
`ACTIVE_CORE` and `SHADOW_EXPERIMENT` cannot be compared, and no promotion gate
can ever be evidenced.

Two temptations exist and both are rejected here. The first is to mark
positions at the midpoint, which reports profit that no counterparty offered.
The second is to infer resolution from a price feed we do not treat as
authoritative elsewhere, and then present it as settlement truth.

ADR 0093 and ADR 0094 scoped the engine and its bridge to BTC. Widening to ETH
does not reverse either decision; the evaluation boundary, the proposal-only
authority, and the performance-book separation are all unchanged.

## Decision

- Value every open paper position at the executable bid for its own side. A
  position is worth what the book will currently pay, never the midpoint and
  never the entry ask.
- Realize P&L only against executable quotes: partial reductions and full exits
  price at the bid, and settlement pays `1.0` or `0.0` per share.
- Settle a window from the observed reference level against the recorded window
  open, journalled with `settlement_source="observed_reference"`. This is a
  paper mark, not the official resolution, and it may never gate real money.
- Extend `MarketSecondSnapshot`, `OpenPosition`, and `DecisionRecord` with the
  asset, so BTC and ETH share one engine and one ledger without merging books.
- Rename the snapshot's reference fields from BTC-specific names to
  asset-neutral ones (`reference_spot_micros`, `window_open_micros`,
  `reference_return_bps`) now that a second asset exists.
- Aggregate a ledger by setup, asset, and window in the runtime, and publish it
  through the worker status file. `ACTIVE_CORE` and `SHADOW_EXPERIMENT` totals
  stay separate at every level of aggregation.
- Keep the crate, binary, and journal directory names from Spec 090. They are
  operator-visible paths under supervision; renaming them buys nothing and
  breaks the managed stack's continuity.

## Consequences

The terminal can show committed stake, live unrealized P&L, realized results,
and per-setup win rates from real journal data, so a strategy can be judged
instead of merely watched. Reported P&L is deliberately conservative: marking
at the bid understates a position that could be worked out at better prices.

Paper settlement now depends on the observed reference level. That dependency
is explicit in the journal so a later reconciliation against the authoritative
resolver can correct the book rather than silently disagree with it. Until that
reconciliation exists, this ledger remains paper evidence only and carries no
live-trading or Paper Week capital authority.
