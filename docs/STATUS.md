# Status Overview

Scannable summary of the platform. For the authoritative narrative, see
[CURRENT_STATE.md](CURRENT_STATE.md); for the phase program, see
[DELIVERY_ROADMAP.md](DELIVERY_ROADMAP.md).

**Milestone:** Phase 5.28 — deterministic frozen 48-hour capture/trade CSV
export (Spec 095) plus the multi-asset (BTC + ETH) every-second window strategy
engine (`btc-window-strategy` / Spec 089) with managed `btc-window-paper` bridge
(Spec 090) and a marked paper book carrying stake, realized and unrealized P&L,
binary settlement outcomes, and per-setup attribution (Spec 091 / ADR 0095),
made durable across restarts by journal replay, atomic position checkpoints, a
single-writer lock, and halt-on-write-failure (Spec 092 / ADR 0096), with
adaptive stake by setup win rate and a 50% profit withdraw checkpoint on the
$1,000 paper principal (Spec 093 / ADR 0097), plus a read-only Poly Advisor MCP
for Claude/ChatGPT research over live + historical paper data (Spec 094 /
ADR 0098), and digest-bound Window Policy Control Plane for all eight window
setups (Spec 096 / ADR 0100 Phase 4) with a schema-driven Strategy Builder
workspace in the terminal,
consolidated strategy-attributed paper trade
history, verified BTC 5m/15m/1h per-second datasets and
single-file CSV derivatives; core-backed Paper Week remains the only managed paper
execution authority for pair capital. The historical evidence is now joined by 24 completed
locally observed BTC-hourly sessions through a checksum-bound atomic
UP/DOWN/reference normalization boundary. The combined dataset contains 260
usable independent sessions. Return, rare-positive, path-classification and
path-net-value challengers all failed whole-session walk-forward validation.
The rolling fixed-point research cycle now runs under an exclusive daily
schedule, reuses identical completed evidence, and exposes only `NO_TRADE` or
`PAPER_AUTO_ADMITTED` to the terminal. Its first managed cycle contains 24
local and 260 combined usable sessions, rejected both challengers, and kept the
paper registry at `NO_ELIGIBLE_MODEL`.

A future model that genuinely passes the frozen unseen-test gates is
automatically added to the paper-only model registry. That admission is
checksum-bound and grants no live, capital, placement, signing or submission
authority. Rejected models are never registered.

The consolidated trade-history export contains 2,481 recorded simulations:
eight verified core paper pair fills, 2,340 retired directional simulator
trades, and 133 retired intraday simulator trades. Strategy provenance and
authority class are explicit on every row. It contains zero real-money trades
and grants no execution or promotion authority. Legacy directional /
fixed-threshold triggers are DISABLED; new BTC 5m/15m/1h paper evaluation is
owned by `btc_window_engine_v1` (5m/15m CORE CANDIDATE, 1h SHADOW).

The first content-addressed BTC five-minute grid contains 382 accepted
contracts and exactly 114,600 second rows across 34 UTC-hour partitions.
Authentic REST observations are carried only with their source/availability
times and ages; missing, stale and one-sided seconds remain explicit. One
same-nanosecond conflicting condition is quarantined and manifest-listed.
All partitions are also available as one separately content-addressed,
checksum-verified 114,600-row CSV. The merge preserves source values and does
not convert carried grid values into claimed fresh ticks.

The 15-minute export contains 128 completed authentic conditions and 115,200
rows. The one-hour export contains 25 completed authentic conditions and
90,000 rows. One active condition per timeframe was excluded. Their merged
CSV derivatives independently preserve exact 900- and 3,600-second grids,
source checksums, fixed-point values and explicit quality state.

**Everything below is offline, read-only, and paper-only.** No crate contains a
credential, private key, wallet action, authenticated transport, signature
engine, or live order-submission path. Live and environment gates are external
and cannot be inferred complete from this repository.

## Legend

| Status | Meaning |
| --- | --- |
| ✅ code-complete | Deterministic implementation + local certification pass (tests, recovery, applicable TLA+ model). |
| ⛔ external gate | Requires real infrastructure evidence, credentials, or operator authorization supplied outside the repo. Not started / cannot be inferred. |

## Current runtime boundary

The terminal can observe public markets and operate one explicit, policy-bound
Paper Week campaign through the Rust core. Legacy Python directional and
intraday bankroll simulators are DISABLED as order triggers and retained only
as historical RETIRED_LEGACY evidence. New BTC multi-window paper decisions are
evaluated by `btc-window-strategy` (proposal-only). They are not an account,
live strategy authority, model, or path to live trading.

The legacy live hourly artifact remains insufficient for independent training.
A separate combined 259-session dataset has now supported controlled offline
model evaluation, including 23 completed local sessions, but no tested
directional challenger demonstrated positive repeatable development P&L.
Failed reports cannot activate the paper or live paths.

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

## Crate inventory (61 crates)

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
| `btc-window-strategy` | Every-second BTC 5m/15m/1h paper engine + managed `btc-window-paper` bridge. |
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
