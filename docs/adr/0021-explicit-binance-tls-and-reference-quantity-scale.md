# ADR 0021: Pin Binance transport and reference-quantity precision

## Status

Accepted for Phase 1.5 closure.

## Decision

The Binance public reference gateway uses rustls with WebPKI roots, explicit TLS
1.2 support, and HTTP/1.1 ALPN for its WebSocket upgrade. This matches the
protocol accepted by the observed `data-stream.binance.vision` edge while
retaining certificate verification and the existing pure-Rust transport.

Binance reference-feed quantities use `ReferenceQuantityE8`, an unsigned
fixed-point integer with 1e-8 scale. Prices remain `QuotePriceMicros`; trading,
accounting, and prediction-token quantities remain in their existing domains.
Reference payload version 2 encodes quantity fields at the new exact scale.
No conversion between the two quantity types is implicit.

## Consequences

Valid Binance quantities are no longer rejected or rounded below one micro.
The type boundary prevents eight-decimal predictive/reference observations from
silently entering six-decimal execution or accounting state. Any future
transport or scale change requires a new compatibility decision and live smoke.
