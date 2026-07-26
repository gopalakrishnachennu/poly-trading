# ADR 0079: Binance spot and verified flow are causal model features

- Status: Accepted
- Date: 2026-07-26

## Decision

The exact Binance BTC/USDT feed used by the market rules is the settlement-path
feature source. Chainlink and other spot feeds are excluded from settlement
truth. Supplemental Polymarket fills enter flow features only after exact
official market/token verification and transaction/log deduplication. All
financial values become integer micros at ingestion.

## Consequences

Intraday challengers can measure distance from the hourly opening price,
momentum, volatility and signed prediction-market flow without confusing
predictive feeds with resolution truth. These features remain research-only and
grant no model, paper or execution authority.
