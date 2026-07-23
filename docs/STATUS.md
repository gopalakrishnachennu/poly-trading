# Status Overview

Scannable summary of the platform. For the authoritative narrative, see
[CURRENT_STATE.md](CURRENT_STATE.md); for the phase program, see
[DELIVERY_ROADMAP.md](DELIVERY_ROADMAP.md).

**Milestone:** Phase 4.7 — code complete and locally integrated.

**Everything below is offline, read-only, and paper-only.** No crate contains a
credential, private key, wallet action, authenticated transport, signature
engine, or live order-submission path. Live and environment gates are external
and cannot be inferred complete from this repository.

## Legend

| Status | Meaning |
| --- | --- |
| ✅ code-complete | Deterministic implementation + local certification pass (tests, recovery, applicable TLA+ model). |
| ⛔ external gate | Requires real infrastructure evidence, credentials, or operator authorization supplied outside the repo. Not started / cannot be inferred. |

## Delivery gates (from the roadmap)

| Phase | Scope | Local code | Environment / live |
| --- | --- | --- | --- |
| ≤ 2.32 | Offline safety baseline (capture → ledger → risk → paper → shadow) | ✅ | n/a (offline) |
| 2.33 | Credential-provider adapter certification | ✅ | ⛔ |
| 3.0 | Durable infrastructure adapters (PostgreSQL/Redpanda/ClickHouse/Parquet) | ✅ | ⛔ real infra soak |
| 3.1 | Production security boundary (signer, Vault/KMS/HSM, workload identity) | ✅ (fakes) | ⛔ real providers |
| 3.2 | Live read-only venue integration | ✅ | ⛔ eligible-network evidence |
| 3.3 | Blockchain + wallet observation (multi-provider RPC) | ✅ (fixtures) | ⛔ real RPC |
| 3.4 | Continuous shadow-operation certification | ✅ (accelerated) | ⛔ real multi-day soak |
| 3.5 | Live-data paper-trading composition | ✅ | ⛔ |
| 3.6 | Authenticated no-submit certification | ✅ | ⛔ |
| 3.7 | Micro-capital canary controls | ✅ (eligibility only) | ⛔ operator + real canary |
| 3.8 | Controlled-production release | ✅ (revocable eligibility) | ⛔ operator + real capital |
| 4.0 | Read-only operator terminal projection | ✅ | n/a (read-only) |
| 4.1–4.7 | Terminal telemetry, research export, immutable config, paper campaigns, model governance, frozen datasets | ✅ | n/a |

## Crate inventory (60 crates)

All crates are single-writer, journal-first (append + fsync before mutate),
strictly replayable, and BLAKE3 checkpointed, with absorbing integrity halts.

### Foundations & market data
| Crate | Role |
| --- | --- |
| `common-types` | Fixed-point financial primitives (micros; no floating point). |
| `event-schema` | Versioned deterministic event envelopes. |
| `public-market-data` | Read-only Polymarket CLOB WebSocket capture + hourly discovery. |
| `order-book-replay` | Deterministic single-writer order-book reconstruction + state digest. |
| `live-market-state` | Bounded actor pipeline for live capture with freshness/readiness gates. |
| `market-recorder` | Journal-integrated read-only recorder. |
| `reference-market-data` | Binance BTCUSDT/ETHUSDT candle + trade + best-price capture. |
| `resolution-rules` | Market-to-oracle resolution contracts and finalized evidence. |

### Feed supervision & sessions
| Crate | Role |
| --- | --- |
| `feed-supervisor` | Cross-feed freshness / clock-integrity supervision. |
| `market-session` | BTC/ETH hourly identity lifecycle (upcoming → finalized). |
| `session-runtime` | Durable single-writer session coordinator. |
| `integration-daemon` | Supervise → frame → durable-coordinate integration owner. |
| `shadow-ops` | Operational supervisor + OpenMetrics rendering. |

### Ledger, risk & offline paper path
| Crate | Role |
| --- | --- |
| `accounting-ledger` | Per-asset double-entry ledger, confirmed-only workflows. |
| `settlement-reconciliation` | Ledger-vs-finalized-chain reconciliation. |
| `portfolio-risk` | Non-bypassable scenario risk authority (APPROVE / NO_TRADE). |
| `order-intent-policy` | Exchange modes, signer-policy frames, placement authorizations. |
| `paper-execution` | Paper order lifecycle + reconciliation handoffs. |
| `paper-trading-runtime` | Single-writer composition of the Phase 2.0–2.4 path. |

### Strategy & paired complete-set arbitrage
| Crate | Role |
| --- | --- |
| `strategy-proposal` | Inert candidate from applied session frame. |
| `complete-set-arbitrage` | Conservative buy/sell-pair top-of-book economics. |
| `paired-opportunity-runtime` | Two-candidate combined scenario risk. |
| `paired-capital-staging` | Transactional both-or-neither reservation. |
| `paired-placement-policy` | Short-lived paper-only leg permissions + hedge ordering. |
| `paired-paper-execution` | One-use permit consumption + fill modeling. |
| `paired-settlement-runtime` | Confirmed-only settlement, pair locking. |
| `ctf-transaction-runtime` | Split / merge / redemption simulation. |
| `unified-paired-trading-runtime` | Domain-only owner of the full nested stack. |

### Shadow certification & campaigns
| Crate | Role |
| --- | --- |
| `shadow-adapter-certification` | Adapter contract + venue fixture certification. |
| `shadow-gateway-harness` | Certification + heartbeat composed with unified runtime. |
| `shadow-session-campaign` | Multi-day hash-chained recorded-session campaigns. |
| `continuous-shadow-certification` | Accelerated continuous soak with resource budgets. |
| `shadow-auth-session` | Opaque authenticated-observation lease lifecycle. |

### Promotion, canary & deployment governance
| Crate | Role |
| --- | --- |
| `promotion-governance` | Canary-eligibility aggregation + dual control. |
| `canary-rollout-simulator` | Health-gated simulated rollout stages + rollback latches. |
| `fleet-rollout-governance` | Region-bound rollout aggregation + change freeze. |
| `deployment-preflight` | Regional provenance + least-privilege + rollback evidence. |
| `deployment-orchestration-simulator` | Regional wave simulation + reverse rollback. |
| `deployment-adapter-certification` | Credentialless control-plane fixture certification. |
| `deployment-change-control` | Sealed maintenance windows + one-use handoffs. |
| `deployment-change-campaign` | Multiple change-control plans + restart equivalence. |
| `production-change-readiness` | Aggregated readiness record (non-executable). |
| `deployment-execution-intent` | Credentialless executor + one-use next-step intent. |
| `executor-session-simulator` | Process-isolation lease protocol simulation. |

### Transport, credential & submission boundaries
| Crate | Role |
| --- | --- |
| `transport-adapter-certification` | Recorded DNS/TLS/endpoint/serialization fixtures. |
| `credential-broker-simulator` | Zero-key signing-policy boundary + permits. |
| `submission-gateway-certification` | Endpoint/request/channel-bound shadow envelopes. |
| `credential-provider-certification` | Opaque-handle acquisition/rotation/revocation certification. |

### Durable infra, security & live-readiness boundaries
| Crate | Role |
| --- | --- |
| `durable-infrastructure` | PostgreSQL/Redpanda/ClickHouse/Parquet contract certification (fakes). |
| `security-boundary` | Workload identity + fake Vault/KMS/HSM + isolated signer. |
| `read-only-venue` | Public/user/metadata/reference channel supervisor. |
| `chain-observer` | Three-provider read-only chain + wallet observation. |
| `live-data-paper-certification` | Point-in-time-correct paper certification over captures. |
| `authenticated-no-submit` | Authenticated observation with absent submit capability. |
| `micro-capital-canary-controller` | Complete-set allowlist + tiny signed ceilings (eligibility only). |
| `controlled-production-release` | Staged capital ceilings + governance (revocable eligibility only). |

### Terminal & research
| Crate | Role |
| --- | --- |
| `terminal-projection` | Credentialless gateway feeding the operator terminal (127.0.0.1:8088). |
| `paper-campaign-schema` | Policy-bound paper campaign records. |
| `model-governance` | Offline research model artifact + walk-forward evidence. |
| `paper-learning-dataset` | Frozen chronological train/validation/test folds. |

## Verification gates

Run before every change (see [../AGENTS.md](../AGENTS.md)):

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo deny --workspace check
```

Formal models (49 bounded TLA+ specs under `formal/`) and the terminal
(`cd terminal && npm run lint && npm test`) are checked in CI
(`.github/workflows/verify.yml`).
