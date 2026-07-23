# ADR 0045: Certify Recorded Transport Before Networking

- Status: Accepted
- Date: 2026-07-21

## Decision

Phase 2.29 certifies endpoint and transport behavior exclusively from sealed
recorded fixtures. It binds a current Phase 2.28 dossier, exact request-template
digests, canonical request bytes, lowercase hostname, port 443, SNI, endpoint
paths and certificate pins.

The fixed matrix proves allowed and denied DNS, TLS, endpoint and serialization
behavior plus conservative timeout, rate-limit and unknown-response handling.
Unknown response evidence must be followed by exact no-mutation reconciliation.
No socket, resolver, TLS stack or HTTP client is introduced.

## Consequences

- Transport policy can be reviewed and replayed without network authority.
- A certificate demonstrates fixture conformance, not connectivity.
- Credentials and actual transport remain future separately authorized work.
- Dynamic endpoints, automatic retries and ambiguous success are prohibited.
