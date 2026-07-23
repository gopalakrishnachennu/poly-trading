# ADR 0009: Explicit hourly resolution contract

- Status: Accepted
- Date: 2026-07-17

## Context

The reference feed can provide many valid Binance candles. A correct candle for
the wrong symbol, hour, interval, or market rule is still incorrect settlement
evidence. Current BTC and ETH hourly rules use the finalized Binance Spot
BTC/USDT or ETH/USDT one-hour candle and resolve equality to `Up`.

## Decision

Introduce a deterministic `resolution-rules` crate between market discovery,
reference replay, and any future settlement or strategy component.

An immutable `ResolutionContract` binds:

```text
condition ID + question ID + token IDs + rules fingerprint
market asset + configured series
exact Binance resolution URL
UTC market start/end
exact Binance symbol and finalized 1h candle
close >= open comparator
```

Binding fails if the source URL or required rule language changes. This is
deliberately conservative: changed market rules require reviewed code and a new
ADR or schema version.

An in-progress candle may produce only `IndicativeAssessment`, whose field is
named `outcome_if_closed_now`. It can never create final evidence. Only the
exact matching `FinalizedCandle` can create checksummed `ResolutionEvidence`.
Equality resolves to `Up`; a lower close resolves to `Down`.

Final evidence is immutable and idempotent. A conflicting second finalized
candle halts without mutating the first result. Its explicit wire encoding
contains market identifiers, winning token, rules fingerprint, symbol, candle
window, open, close, outcome, and a BLAKE3 checksum.

## Consequences

- A title, display price, other exchange, other pair, adjacent candle, or open
  candle cannot be mistaken for resolution evidence.
- Rule changes fail closed instead of silently changing semantics.
- Computed evidence does not mean Polymarket has proposed, confirmed, or paid
  the resolution. Those external lifecycle states remain future work.
- No authenticated API, wallet, order, or strategy capability is introduced.
