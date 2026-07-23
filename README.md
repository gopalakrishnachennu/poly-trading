# Poly Trading

An institutional-style, event-driven prediction-market trading platform built
around deterministic state, conservative accounting, replayability, and
non-bypassable risk controls.

This repository has completed **Phase 4.0** at the local integration level. A
credentialless Rust projection gateway now drives the Bloomberg-style terminal
from validated current public BTC/ETH hourly identities, exact complementary
books and reference prices. It is deliberately `NO_TRADE`: it has no accounting,
risk, credentials, signing or order authority, and clears the complete display
when any required source is invalid or stale. The offline Phase 2 safety program
is closed; the durable-data boundary certifies PostgreSQL, Redpanda, ClickHouse
and Parquet-compatible contracts, and the production security boundary now
certifies workload identity, fake Vault/KMS/HSM providers and isolated-signer
least privilege without secrets or external connections. A read-only venue
supervisor now composes independently healthy public, user, metadata and
reference channels with dynamic parameters, exchange modes and strict reconnect
recovery. In addition to the read-only hourly
market, reference, supervision, session, replay, soak, and operational stack, it
now includes strict three-provider read-only blockchain and wallet observation.
Finalized block/hash and canonical wallet state must agree across every provider;
provider staleness, chain substitution, finalized equivocation and incomplete
reorg recovery fail closed. Collateral, allowance, CTF balances and transaction
states remain separate fixed-point facts, and pending or mined effects are never
promoted to finalized spendable state. In addition to that observation stack, it
includes an accelerated continuous shadow-certification campaign with strict
resource budgets, contiguous rollover, restart/partition/dead-man recovery and
dual-operator accountability. Accelerated logical duration is recorded
separately and never represented as a real multi-day target-environment soak.
The platform also
contains point-in-time-correct paper certification over captured sessions.
Event, receive and strategy-available time are separate; walk-forward folds are
chronological, final-test strategy identity is frozen, and passive fills require
queue evidence across optimistic, estimated and conservative cases. Results are
paper evidence only and never represent real fills or P&L. It also
certifies an opaque authenticated-observation lifecycle with predecessor-bound
rotation, revocation, outage/dead-man/unknown reconciliation and distinct-region
disaster recovery. No credential or connection is created: physical submit
capability is absent and mutation purposes are independently denied. It also
contains the `micro-capital-canary-controller`, which binds that exact report
to an immutable complete-set allowlist, tiny signed fixed-point ceilings,
distinct risk/operations approvals and deterministic capital-floor, loss,
exposure, kill, dead-man, abort and rollback cases. Its output is code
eligibility only: no capital is allocated and every live authority remains
false. The final `controlled-production-release` layer binds that report to
immutable release subjects, strictly increasing signed fixed-point ceilings,
three-person governance, independently fresh multi-region health, continuous
reconciliation and deterministic expiry, incident, disaster-recovery, rollback
and revocation evidence. It grants code eligibility only; real production
completion and every external authority remain false. It also
contains an offline deterministic double-entry ledger plus a settlement kernel
and a non-bypassable scenario portfolio-risk authority. It reconciles immutable
local trade facts, CLOB lifecycle state, ledger postings, and finalized
blockchain balances before evaluating fill permutations, terminal outcomes,
correlated shocks, capital haircuts, and hard exposure limits. An independent
offline intent-policy layer then binds exact approvals to exchange modes,
signer-policy constraints, replay protection, and delayed/cancel lifecycle.
The paper-execution kernel exercises ambiguous acknowledgements, unknown
state, partial/full fills, delayed rejection, cancel races, retry classification,
and settlement-reconciliation handoffs deterministically. A single-writer
`paper-trading-runtime` now composes the Phase 2.0–2.4 authorities into one
journal-first pipeline: current reconciled state, risk approval, exact capital
reservation, intent policy, paper execution, unique settlement handoff,
confirmed accounting, and final reconciliation cannot be reordered or
substituted across component boundaries. Deterministic one-shot faults,
restart/checkpoint replay, confirmed-fill integration, and multi-hour unknown-
recovery soaks exercise the composed state machine.
At the production-change boundary, `deployment-execution-intent` now consumes a
current exact Phase 2.26 readiness record, certifies a credentialless isolated
executor against a fixed denial matrix, and emits only short-lived one-use
next-step manual handoff intents under exact operation, resource-digest and
region ceilings. It has no signer, secrets, authenticated transport or external
submission path.
The `executor-session-simulator` adds the next offline protocol layer: exact
Phase 2.27 evidence binding, credentialless process isolation, exclusive leases,
ordered request envelopes, simulated acknowledgement/unknown outcomes,
dead-man expiry, restart recovery and mandatory no-mutation reconciliation.
The `transport-adapter-certification` layer now certifies recorded DNS, TLS pin,
endpoint, canonical serialization, timeout, rate-limit and unknown-response
fixtures without creating a resolver, socket, HTTP client or retry path.
The `credential-broker-simulator` adds a zero-key signing-policy boundary over
that exact certificate. Opaque attested handles, exact purpose/subject/integer
unit/time ceilings, a fixed signer failure matrix, distinct security and
operations authorization, short-lived one-use permits, unique nonces,
revocation and digest-only receipts are deterministic and replayable. It
contains no key material, real signature algorithm, provider client,
credential, authenticated transport or external-submission path.

## Run the live read-only terminal

```text
cd terminal
npm install
npm run dev:full
```

Open `http://localhost:3000`. The Rust projection gateway binds only to
`127.0.0.1:8088`; the UI accepts only atomic, fresh schema-v1 BTC+ETH snapshots.
See `docs/runbooks/terminal-projection.md` for failure and rollover behavior.
The `submission-gateway-certification` layer now binds that exact receipt chain
back to the originating transport endpoint and canonical request bindings.
Opaque channel/token subjects, bounded envelopes, unique idempotency identities,
recorded gateway fixtures, exactly-once shadow staging and mandatory unknown-
response no-mutation reconciliation are deterministic and replayable. It has no
credential values, signature engine, socket, HTTP client or external submission.
The `shadow-auth-session` coordinator now adds exclusive expiring leases,
monotonic recorded heartbeats, predecessor-bound attestation rotation, dead-man
and unhealthy revocation, restart invalidation and exact ambiguity recovery.
Recovery returns idle and never reopens a lease. The layer remains entirely
offline and contains no credential, private key, provider or transport client.
An independent `strategy-proposal` boundary now captures exact applied-session
provenance and can turn a bounded fixed-point intent into only an inert risk
candidate. Candidate creation requires the current ready session and
authoritative complementary books; it cannot approve risk, reserve capital,
permit placement, sign, connect, or submit.
The `complete-set-arbitrage` detector now evaluates conservative buy-pair and
sell-pair top-of-book economics. It applies explicit worst-case fees and
conversion cost, conservative per-leg rounding, liquidity caps, and profit/ROI
thresholds before producing exactly two inert proposal intents. A detected plan
is not considered locked profit until downstream fills and settlement prove it.
The `paired-opportunity-runtime` now derives both Phase 2.6 candidates itself and
runs them through one combined Phase 2.2 scenario product. Both legs share
capital capacity and have independent zero/partial/full fill states. Paired risk
eligibility uses a distinct digest that cannot authorize either individual leg.
The `paired-capital-staging` runtime binds that evaluation to its owned ledger
and installs both exact capital reservations transactionally or installs
neither. Its paired abort releases both reservations together. A staged pair is
still inert: it does not permit placement, signing, transport, or submission.
The `paired-placement-policy` owner can now issue exact, one-second-bounded
paper-only leg permissions. It permits a complementary hedge only after the
first leg is recorded fully matched, retains both reservations across expiry or
any ambiguous/exposure-bearing lifecycle, and allows paired abort only when
both legs prove zero possible fill. These permissions are audit facts, not live
orders or signatures.
The `paired-paper-execution` owner now consumes those permits exactly once and
models submission, delayed and unknown outcomes, acknowledgements, live orders,
partial/full fills, cancellation races, rejection, and recovery. Each accepted
fill produces one immutable settlement-reconciliation handoff, but reservations
remain locked and no accounting or settlement confirmation is inferred.
The `paired-settlement-runtime` owner now registers only those stored handoffs,
tracks matched/mined/retrying/confirmed/failed settlement, derives exact
confirmed-only ledger postings, and reconciles against finalized paper-chain
assets. Residual reservations release transactionally across both legs only
after terminal execution and current reconciliation. Confirmed complementary
inventory may be locked, but no merge or spendable proceeds are inferred.
The `ctf-transaction-runtime` now simulates split, merge, and redemption as
durable requested/pending/retrying/confirmed/failed transactions. Inputs are
reserved or locked before pending state, duplicate submission is non-posting,
and confirmed accounting is derived exactly once from immutable request facts.
No chain transaction is actually submitted.
The `unified-paired-trading-runtime` now owns that entire nested stack behind
one domain-only command language. It derives reconciliation and ledger risk
provenance, nested command identities, issued permits, fill accounting IDs, and
confirmed-posting subjects internally. Authorization plus paper submission is
transactional, and top-level journal recovery reproduces the complete nested
state.
The `shadow-adapter-certification` authority now validates immutable adapter
contracts, recorded venue fixtures, signer-policy dry runs, deployment-region
eligibility, and synthetic collateral/allowance/gas/relayer conditions. It
requires mandatory adverse scenarios and always emits a non-authorizing audit
report. It contains no network, authentication, key, wallet, RPC, relayer-client,
order, transaction, or deployment path.
The `shadow-gateway-harness` now composes a fresh non-authorizing certification
report and complete-stack synthetic heartbeat with the unified Phase 2.14
runtime. It alone derives simulated exchange modes, translates all recorded
adapter fixtures, activates dead-man behavior on heartbeat failure, requires
reconciliation and unknown-order clearance for restart recovery, and disables
new shadow exposure at certification expiry. Its top-level journal and
checkpoint reproduce the entire nested runtime digest without adding network,
credential, signer, wallet, RPC, relayer, or live-submission capability.
The `shadow-session-campaign` runner now seals multi-day recorded-session and
fault schedules into hash-chained manifests before replay. It derives
certification renewal/expiry, partition, dead-man, heartbeat-loss, restart and
unknown-state recovery coverage from authentic Phase 2.16 outcomes, verifies
terminal readiness and unresolved backing, and writes canonical checksummed
operator evidence. Even eligible evidence requires an explicit operator
decision and grants no promotion, deployment or live authority.
The `promotion-governance` owner now aggregates canonical independent campaign
bundles, enforces freshness, identity diversity and upward-rounded regression
retention, and seals exact source, binary, toolchain, dependency-lock, SBOM,
configuration and rollback digests. Distinct risk and release operators must
approve the unchanged subject. Its checksummed canary-eligibility record still
grants no execution, promotion, deployment, credential or live authority.
The `canary-rollout-simulator` now consumes one exact unexpired eligibility
record and matching rollback criteria. It gates increasing simulated rollout
stages on current independent health and maintenance windows, pauses on ordinary
degradation, irreversibly latches severe rollback triggers, and requires
post-restart health plus explicit operator resume. Its final report cannot route
traffic, execute rollback, deploy, authenticate or trade.
The `fleet-rollout-governance` owner now aggregates exact region-bound Phase
2.19 reports under one sealed release, artifact, rollback and change-freeze
subject. It deduplicates reports and rollout plans, requires every planned
region plus independent abort/rollback drills and configured trigger diversity,
excludes stale evidence, and supports irreversible exact-subject revocation.
Its canonical readiness dossier is attributable and checksummed but cannot
route traffic, deploy, execute rollback, access credentials or trade.
The `deployment-preflight` owner now consumes a renewed current Phase 2.20
binding and seals exact regional image/configuration/infrastructure/network,
credentialless least-privilege and rollback-package evidence. Release, risk and
operations approvals must bind that package and use three distinct opaque
operators. Even a passing report creates no credential and grants no signing,
deployment, rollback, cloud-control or live-trading authority.
The `deployment-orchestration-simulator` now consumes that exact current report
and seals complete, non-overlapping regional waves. Contiguous regional health
gates control simulated start and advance; ordinary degradation pauses, while
reconciliation, capital-floor, timeout or post-activation abort failures latch
reverse-order rollback. Restart preserves progress and never resumes work
automatically. Its checksummed report is evidence only and cannot authenticate,
contact a control plane, deploy, route, execute rollback or trade.
The `deployment-adapter-certification` owner now binds exact Phase 2.22
completion and reverse-rollback evidence to a credentialless adapter contract.
It requires a contiguous recorded control-plane fixture matrix in every region,
mandatory least-privilege denials, and bounded regional, control-plane,
durable-state and artifact recovery drills. Certification is checksummed
evidence only: it cannot load credentials, authenticate, mutate infrastructure,
shift traffic, fail over, roll back or trade.
The `deployment-change-control` owner now binds one exact current Phase 2.23
certificate to sealed ordered maintenance windows, contiguous region-bound
change steps and emergency policy. Distinct current release/risk operators are
required before it can issue a short-lived, one-use manual handoff for the next
step. Pause invalidates outstanding handoffs; post-handoff abort or severe
signals irreversibly require exact reverse rollback handoffs. Completion,
safe-abort and rollback reports remain evidence only and grant no credential,
deployment, cloud-control, rollback or live-trading authority.
The `deployment-change-campaign` owner now seals multiple independent Phase
2.24 plans and their complete child command schedules. Each case runs through a
fresh authentic change-control owner; multi-window, approval renewal/expiry,
pause/resume, safe abort and emergency rollback coverage comes only from child
outcomes. Declared restart points rebuild the exact child prefix and require
complete digest equality. Its checksummed operator-review evidence remains
non-authorizing and cannot authenticate, deploy, route, roll back or trade.
The `production-change-readiness` owner now aggregates canonical Phase 2.25
evidence under an exact release, binary, configuration, infrastructure,
observability, certificate, preflight, plan and rollback subject. It prevents
duplicate inflation, enforces freshness, independent campaign/manifest/
schedule/result/plan diversity and upward-rounded regression floors, and
requires distinct release/risk/operations decisions. A positive record remains
non-executable and grants no deployment or live authority.
The Phase 1.5 Binance public live-smoke gate passed on 2026-07-17; its transport,
synchronization, shutdown, journal, and replay evidence is recorded locally.
It does not submit orders, hold production credentials, or promise returns.

## Current scope

- Fixed-point financial primitives
- Versioned deterministic event envelopes
- Checksummed append-only journal
- Safe recovery from truncated journal tails
- Formal capital-reservation model
- Property and recovery tests
- Bounded BTC/ETH hourly market discovery
- Validated public CLOB WebSocket capture
- Explicit synchronization epochs and rollover rediscovery
- Strict fixed-point public-event decoding
- Deterministic single-writer order-book replay
- Stable replay-equivalence state digest
- Journal-first bounded live-state delivery
- Single-writer actor with freshness and readiness gates
- Recoverable snapshot invalidation for transient book inconsistencies
- Bounded-memory journal and segmented replay
- Deterministic byte/record rotation and directory validation
- Versioned checksummed replay checkpoints
- Exact fixed-point BTCUSDT/ETHUSDT reference-feed normalization
- Type-separated finalized settlement candles and predictive observations
- Transactional reference replay with immutable finalized candles
- Exact market-to-oracle resolution contracts
- Checksummed final evidence with explicit non-final indicative assessments
- Deterministic cross-feed freshness and clock-integrity supervision
- Replay-equivalent readiness digest with permanent history-integrity halts
- Deterministic per-asset double-entry accounting
- Partially consumable collateral and token reservations
- Confirmed-only buy, sell, and merge workflows
- Separate fee, inventory-cost, realized-P&L, and locked-P&L accounting
- Content-bound idempotency with conflicting-key integrity halt
- Journal-first ledger replay and checksummed prefix checkpoints
- Property and TLA+ conservation verification
- Documented MATCHED/MINED/RETRYING/CONFIRMED/FAILED lifecycle enforcement
- Immutable terminal trade facts and transaction-hash continuity
- Exact ledger-versus-finalized-chain collateral and token reconciliation
- Confirmed-posted and failed/unconfirmed-unposted consistency gates
- Bounded confirmation-to-ledger grace and absorbing discrepancy halts
- Journal-first reconciliation replay and checksummed prefix checkpoints
- Digest-bound reconciled ledger risk views with categorized asset haircuts
- Exact open-buy cash and open-sell token reservation backing
- Bounded zero/partial/full fill, terminal-outcome, and correlated-shock products
- Conservative worst-case terminal wealth and exposure witnesses
- Hard capital-floor, gross, condition, and shock-group limits
- Durable attributable `APPROVE`/`NO_TRADE` decisions with integrity halts
- Journal-first portfolio-risk replay and checksummed prefix checkpoints
- TLA+ proof that approval cannot bypass reconciliation, scenario completeness,
  capital floor, exposure limits, or an absorbing halt
- Exact Phase 2.2 candidate-order fingerprints and verifiable decision digests
- Explicit normal, restart, post-only, cancel-only, disabled, recovering, and
  unknown exchange modes
- One-use, expiring risk-approved placement authorizations
- Inert venue/contract/token/quantity/price/notional signer-policy frames
- Delayed, live, cancel-authorized, and terminal paper order lifecycle
- Exact uncancellable-window and delayed-release enforcement
- Journal-first order-intent replay and checksummed prefix checkpoints
- TLA+ proof of placement prerequisites, replay prevention, cancellation safety,
  and absorbing halt
- Deterministic submitted, delayed, acknowledged, live, partial, unknown,
  cancel-pending, fully matched, canceled, and rejected paper states
- Immutable exchange order identity and monotonic observation history
- Explicit cancel-before-fill, fill-before-cancel, and cancel-rejection races
- Exact delta/cumulative fixed-point match validation against original limits
- One immutable Phase 2.1 reconciliation handoff per accepted paper fill
- Descriptive permanent/restart/rate-limit/balance/delay/unknown retry classes
- Journal-first paper execution replay and checksummed prefix checkpoints
- TLA+ proof of non-terminal unknown state, fill/handoff equality, terminal
  exclusivity, cancel-race fills, and absorbing halt
- One single-writer owner for accounting, reconciliation, portfolio risk,
  intent policy, paper execution, and settlement handoffs
- Exact runtime-derived ledger/reconciliation provenance at the risk boundary
- Exact approved-order reservation before placement authorization
- Exact last-decision binding from risk to policy and policy to execution
- Transactional delayed/live/terminal synchronization back into policy state
- Unique execution-derived reconciliation handoffs; caller registration denied
- Confirmed ledger postings bound to unconsumed exact execution handoffs
- Active, unknown, matched, and unposted partial-fill reservations unreleasable
- Deterministic durable one-shot faults before risk, execution, and handoff
- Journal-first composed replay with checksummed prefix checkpoints
- Confirmed-fill integration from reservation through finalized-chain equality
- Deterministic multi-hour unknown/rejection recovery with no capital leakage
- TLA+ proof of composed ordering, handoff uniqueness, one-shot fault use,
  child-halt propagation, and absorbing runtime halt
- Exact applied-frame strategy context with session, feed, and book provenance
- Current-ready, context-expiry, token, and bounded fixed-point proposal gates
- Boundary-derived inert Phase 2.2 candidates with one-use proposal identities
- Journal-first proposal replay and checksummed prefix checkpoints
- TLA+ proof that proposals cannot grant downstream authority or bypass readiness
- Conservative fixed-point buy-pair and sell-pair complete-set detection
- Explicit fee, conversion, liquidity, net-profit, and ROI thresholds
- Exactly two derived inert proposal intents without an atomic-fill assumption
- Journal-first arbitrage replay and checksummed prefix checkpoints
- TLA+ proof of paired output, readiness, no-authority, replay, and halt safety
- Combined two-candidate cash/token capacity and scenario evaluation
- Independent fill permutations for both legs in one portfolio-risk decision
- Non-substitutable detector-to-proposal-to-risk composition
- Pair digest deliberately incompatible with single-order placement authority
- Journal-first paired-runtime replay and checksummed prefix checkpoints
- TLA+ proof of ordered composition, child-halt propagation, and no execution
- Exact Phase 2.8 evaluation bound to the current owned-ledger risk view
- Transactional two-leg reservation with no observable partial stage
- Exact reservation identity, asset, amount, and post-ledger digest attestation
- Both-leg abort/release with deterministic second-leg failure testing
- Journal-first staging replay and checksummed prefix checkpoints
- TLA+ proof of both-or-neither staging and permanent lack of placement authority
- Stage-, candidate-, reservation-, role-, and mode-bound paper permissions
- First-leg then post-full-match complementary hedge sequencing
- Expiry, ambiguity, partial-fill, unknown, and no-fill policy lifecycle
- Both-reservation retention until a provably safe transactional paired abort
- Journal-first paired-policy replay and checksummed prefix checkpoints
- TLA+ proof of hedge ordering, unsafe-abort prevention, and no live authority
- Exact one-use paired-permit consumption under one execution/policy owner
- Caller-inaccessible lifecycle synchronization from simulated execution only
- Delayed, acknowledged, live, partial, unknown, cancel-pending, matched,
  canceled, and rejected paired paper states
- Conservative limit, fee, cumulative fill, identity, and source-history checks
- One unique immutable reconciliation handoff for every accepted fill
- Both-reservation equality across every execution transition
- Journal-first paired execution replay and checksummed prefix checkpoints
- TLA+ proof of permit use, hedge order, fill/handoff equality, and retention
- Stored-handoff-only paired settlement and confirmed-only accounting
- Exact finalized-chain reconciliation, complete-pair locking, and finalization
- Requested, pending, retrying, confirmed, and failed CTF transaction lifecycle
- Confirmed-only split, merge, and redemption with exact failure policies
- One domain-only owner from paired opportunity evaluation through CTF finality
- Runtime-derived ledger and reconciliation provenance at evaluation
- Internally derived child, permit-consumption, fill-ledger, and posting subjects
- Transactional authorization plus submission with injected-boundary rollback
- Top-level journal replay and complete nested-digest prefix checkpoints
- Full paired fill, settlement, pair-lock, merge, and finalization composition
- Pending-conversion restart recovery with exact inaccessible backing
- Twenty-four-hour no-trade and eight-hour split/merge conservation soaks
- Bounded TLA+ proof of ordered authority, atomic submission, confirmed-only
  posting, merge backing, no live authority, and halt absorption
- Immutable shadow-adapter interface contracts and opaque evidence digests
- Mandatory restart, mode, delay, tick, rate-limit, unknown-order, settlement,
  and heartbeat fixture coverage with deterministic safe responses
- Signer-policy dry runs with baseline permit and mandatory denial coverage
- Independent fresh eligibility gates for primary and failover regions
- Separate synthetic collateral, allowance, gas, relayer, and queue-depth gates
- Mandatory failure simulations mapped only to non-exposure actions
- Explicit non-authorizing `CERTIFIED` and attributable `NOT_CERTIFIED` reports
- Journal-first certification replay and complete-state prefix checkpoints
- Property proof that insufficient allowance or gas never certifies
- Bounded TLA+ proof of complete-evidence certification, safe failure actions,
  no authority, and absorbing halt

## Read-only recorder

Inspect currently eligible market identities without creating a journal:

```bash
cargo run --locked -p public-market-data -- --discover-only
```

Record public events until interrupted:

```bash
cargo run --locked -p public-market-data -- ./public-market.journal
```

The executable uses public endpoints only. It contains no API-key, wallet,
signing, or order-submission path.

## Read-only replay

Reconstruct a clean journal and print its stable state digest:

```bash
cargo run --locked -p order-book-replay -- ./public-market.journal
```

Replay halts on journal corruption, incomplete tails, sequence gaps, invalid
epoch transitions, malformed fixed-point values, or order-book invariant
violations.

## Read-only live state

Capture public data through the bounded actor pipeline:

```bash
cargo run --locked -p live-market-state -- ./live-market.journal
```

On clean shutdown the executable prints the terminal live-state digest. Running
the replay command against the same journal must produce the identical digest.

Capture into bounded segments:

```bash
cargo run --locked -p live-market-state -- --segments ./market-segments
```

Stream-replay those segments:

```bash
cargo run --locked -p order-book-replay -- --segments ./market-segments
```

Create and verify a new checkpoint:

```bash
cargo run --locked -p order-book-replay -- \
  --write-segment-checkpoint ./market-segments ./state.checkpoint

cargo run --locked -p order-book-replay -- \
  --segments-checkpoint ./market-segments ./state.checkpoint
```

## Read-only reference feed

Capture Binance's public market-data-only BTCUSDT and ETHUSDT one-hour candles,
aggregate trades, and best bid/ask into a journal:

```bash
cargo run --locked -p reference-market-data -- ./reference-market.journal
```

Replay the same journal and print its deterministic digest:

```bash
cargo run --locked -p reference-market-data -- --replay ./reference-market.journal
```

Only finalized UTC one-hour candles are settlement-reference evidence.
Aggregate trades, best prices, and in-progress candles remain predictive data.

## Build

The repository pins its Rust channel in `rust-toolchain.toml`.

```bash
cargo fmt --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Read [AGENTS.md](AGENTS.md) before making changes.
