# Phase 1.5 Binance Public Live-Smoke Evidence

## Result

Passed on 2026-07-17 at approximately 11:08 UTC from the project developer
workstation network. Binance accepted the public endpoint and delivered the
complete configured BTCUSDT/ETHUSDT feed set without authentication.

This proves technical public-data eligibility for this network at the recorded
time. The deployment operator remains responsible for confirming legal and
geographic eligibility of every future production region.

## Endpoint and transport

- Endpoint: `wss://data-stream.binance.vision/stream`
- Streams: BTCUSDT and ETHUSDT `kline_1h`, `aggTrade`, and `bookTicker`
- DNS/TCP: successful
- Certificate verification: successful against WebPKI roots
- Negotiated protocol: TLS 1.2 with HTTP/1.1 WebSocket upgrade
- Raw upgrade probe: HTTP `101 Switching Protocols`
- Public REST control probe: HTTP `200` from `data-api.binance.vision/api/v3/ping`

The pre-smoke failure was not a geographic rejection. Workspace rustls defaults
had compiled out TLS 1.2, while the Binance edge selected TLS 1.2. The gateway
now enables rustls `tls12` explicitly and advertises HTTP/1.1 ALPN.

## Precision correction

The first successful transport exposed valid Binance quantities with non-zero
digits at 1e-8 precision. Reference quantities now use a separate fixed-point
`ReferenceQuantityE8` type. Trading quantities and accounting remain at their
existing six-decimal scale. Normalized reference payload version 2 records the
new exact quantity scale without rounding.

## Capture and replay

- Durable start marker: present once
- Durable synchronized marker: present once
- Durable shutdown marker: present once
- Terminal health: `Shutdown`
- Terminal sequence: `1604` (1,605 events numbered from zero)
- Journal size: approximately 190 KiB
- Journal SHA-256: `bdc7f16c4796be94ecfab68b2b76dce1a6b792ce49da432d21093c606e94f2e5`
- Replay digest: `72aa144f45bed729c4ed73ffcb2a47094fda15c974d46aa9458f8154c9bbfbb3`

The synchronized marker is emitted only after candle, aggregate-trade, and
book-ticker observations have all arrived for both configured symbols in the
same epoch. Shutdown was requested with SIGINT and journaled before the socket
closed. Strict replay accepted the complete journal and reproduced terminal
state deterministically.

## Scope

No API key, credential, private key, authenticated API, wallet, strategy,
automatic retry beyond the existing public capture reconnect loop, order, or
live trading capability was used or added.
