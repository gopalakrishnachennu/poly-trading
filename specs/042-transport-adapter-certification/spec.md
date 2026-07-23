# Phase 2.29 Specification: Offline Transport-Adapter Certification

## Objective

Consume one current Phase 2.28 dossier and certify a credentialless transport
adapter policy using recorded DNS, TLS, endpoint, serialization, timeout,
rate-limit, unknown-response and reconciliation fixtures without opening a
socket or submitting a request.

## Scope

- Versioned Phase 2.28 dossier template-digest provenance.
- Exact lowercase hostname, port, SNI, endpoint-path and certificate-pin policy.
- Canonical request serialization bindings for every upstream template.
- Fixed recorded-fixture matrix with deterministic expected dispositions.
- Conservative timeout/rate-limit backoff and unknown-response reconciliation.
- Journal-first commands, strict replay, checkpoints and certification files.

## Exclusions

- DNS lookup, socket creation, TLS handshake or HTTP client execution
- Credentials, cookies, authorization headers, signatures or private keys
- Proxies, redirects, dynamic endpoints or wildcard certificates
- External submission, deployment, wallet, exchange or live trading activity

## Acceptance criteria

- Invalid, stale, incomplete, corrupt, substituted or authority-bearing Phase
  2.28 evidence fails closed before registration.
- Endpoint policy is exact, canonical and HTTPS-only on port 443; wildcard host,
  redirects, proxies, cookies, query credentials and authentication are denied.
- Nonzero canonical request bindings cover every exact upstream template once.
- DNS, TLS pin, endpoint and serialization allow/deny fixtures validate their
  actual recorded fields rather than only caller labels.
- Timeout and rate-limit fixtures produce bounded backoff, never automatic retry.
- Unknown response requires reconciliation, followed immediately by exact
  recorded no-mutation reconciliation evidence.
- Any credential, signature, socket, authenticated request, external submission
  or mutation claim halts absorbingly.
- Certification requires the complete fixed fixture matrix in order.
- Commands and evidence are bounded, versioned, content-idempotent,
  journal-first, checkpointed, create-new and checksummed.
- Tests and TLA+ cover identity, pins, serialization, ambiguity, reconciliation,
  no-authority and absorbing halt invariants.
