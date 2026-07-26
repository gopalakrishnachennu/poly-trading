# ADR 0089: Five-minute research grid preserves missing seconds

- Status: Accepted
- Date: 2026-07-26

## Decision

BTC five-minute REST capture is converted into an exact 300-row event-time
grid per contract. The grid may carry the most recent authentic Up and Down
books, but each carried value retains its original batch-start time, derived
causal-availability time and age. Missing and stale seconds remain explicit.

The legacy collector timestamp was taken before sequential Up/Down HTTP
requests completed. The exporter therefore adds a versioned conservative
availability delay before either response can become causal model input.

## Consequences

The dataset is visually complete without pretending that a fresh venue event
arrived every second. Models can filter on `market_ready`, observation ages and
quality flags. A complete 300-row grid is not a claim of complete raw capture,
fillability, profitability or financial authority.
