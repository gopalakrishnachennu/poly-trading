# ADR 0032: Compose a Certified Shadow Gateway Without Credentials

## Status

Accepted for Phase 2.16.

## Decision

Introduce `shadow-gateway-harness` as the single credentialless owner above the
Phase 2.14 unified offline runtime. The harness accepts a digest-valid Phase
2.15 report, complete-stack synthetic heartbeats, recorded fixtures, explicit
recovery evidence, and Phase 2.14 domain commands.

New shadow exposure is admitted only while the report and heartbeat are fresh,
healthy, and the derived exchange mode is placement-capable. The harness alone
derives Phase 2.14 exchange-mode observations. Certification or heartbeat
expiry derives `TRADING_DISABLED`; restart recovery derives `RECOVERING` and
then `NORMAL` only after current reconciliation and unknown-order clearance.

All fixtures are inert recorded evidence. They can disable, restrict, retain,
or require reconciliation, but cannot retry automatically or release backing.
The harness journals top-level commands and checkpoints its complete nested
digest.

## Consequences

Certification freshness and dead-man behavior are now exercised against the
real offline orchestration boundary without introducing an authenticated
adapter. The crate has no credential, private key, signature, HTTP, WebSocket,
RPC, wallet, relayer client, deployment, or live order/transaction capability.
