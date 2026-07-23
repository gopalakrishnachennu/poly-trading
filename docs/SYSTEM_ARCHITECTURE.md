# System Architecture

## Architectural shape

The production target is a small deterministic Rust trading kernel surrounded
by independent safety and asynchronous data systems.

```text
Feeds -> normalization -> market actor -> scenarios -> local risk -> execution
                    \-> append-only journal -> replay/research/audit
```

Each market has exactly one state owner. Parallelism occurs across markets, not
through concurrent mutation of a single market.

### `model-governance`

Owns the offline deterministic boundary between adaptive research and paper
promotion. It validates immutable model, feature-schema, configuration, code,
training-fold and evaluation evidence. Chronological train/validation/test folds
are disjoint; a model must freeze before the unseen test fold. Independent
research, evaluation and adversarial labels are required.

```text
captured paper evidence -> model artifact + walk-forward folds
                      -> champion/challenger governance -> paper candidate / NO_TRADE
```

The output binds policy and evidence digests but cannot reserve capital, approve
risk, place, sign or submit an order. Missing provenance, stale models, model
drift, insufficient data, failed adversarial review or worse conservative net
performance falls back to `NO_TRADE`.

### `paper-learning-dataset`

Owns bounded conversion of completed paper JSONL journals into immutable,
replay-verifiable research datasets. Each source record is digest-checked,
campaign-bound, contiguous and timestamp-validated before its event, receive
and strategy-available time enters a fold manifest.

```text
paper journal -> record digest/sequence validation -> equal-time buckets
              -> chronological train/validation/test folds -> artifact binding
```

The freezer rejects corruption, duplicate records, mixed campaigns, invalid
timestamps and insufficient chronology. A model artifact can bind to one exact
dataset but receives no promotion, capital, risk, signing or execution
authority.

## Implemented components

### `common-types`

Owns validated fixed-point primitives and conservative arithmetic. It contains
no network, storage, or strategy logic.

### `event-schema`

Owns the versioned canonical event envelope. Encoding is explicit and
deterministic rather than derived from in-memory Rust layout.

### `market-recorder`

Appends encoded envelopes to checksummed journal segments. Recovery accepts
clean records, may truncate an incomplete tail, and halts on checksum or format
corruption.

The bounded-memory `JournalReader` decodes one record at a time. The legacy
collecting scan is a compatibility wrapper over that reader. A segmented writer
rotates by configured bytes or records into contiguous files named:

```text
segment-00000000000000000000.journal
segment-00000000000000000001.journal
...
```

Segment directories reject gaps, sequence discontinuity, symlinks, unexpected
entries, corruption, and incomplete tails. Single and segmented writers expose
the same minimal append/sync capture boundary.

### `public-market-data`

Owns the read-only external boundary for configured BTC and ETH hourly markets.
It performs bounded Gamma keyset discovery, validates immutable market identity
and rules, subscribes to the public CLOB WebSocket by validated token IDs, and
journals each accepted public event independently.

The boundary has no authenticated channel, wallet, signer, order, cancellation,
or position capability. A connection is one synchronization epoch. Disconnect,
periodic market rollover, or heartbeat failure ends the epoch and forces market
rediscovery; cached books never cross an epoch as authoritative state.

```text
Gamma keyset -> identity validation -> public CLOB subscription
                                      -> event validation -> journal
```

Gamma recurring-event creation time is not the trading-hour start. Discovery is
bounded by market resolution time, then the exact interval is validated from
`market.eventStartTime` and `market.endDate`.

### `order-book-replay`

Owns strict public-payload decoding and the deterministic single-writer replay
state. It converts external decimal strings directly into integer micros,
requires contiguous recorder sequences, invalidates state across connection
epochs, and requires a fresh snapshot before token deltas.

Each condition/token pair is stored in canonical ordered maps. Snapshot, delta,
tick-size, last-trade, and best-price events pass through the same typed decoder.
The state digest uses explicit field encoding and sorted iteration; it does not
hash Rust memory layout.

Replay streams both single and segmented journals. Versioned BLAKE3-checksummed
checkpoints encode complete replay state explicitly. A checkpoint must match its
durable sequence prefix and digest before later events are applied; it is never
an independent source of authority.

```text
checksummed journal -> typed fixed-point events -> epoch gate
                    -> condition/token books -> stable state digest
```

### `live-market-state`

Owns the bounded Tokio wrapper around the deterministic replay state. Public
capture appends an envelope before attempting non-blocking channel delivery.
Full or closed ingress ends capture; no event is silently dropped.

The actor is the only live-state writer. Readers receive immutable watch
snapshots containing readiness, epoch, sequence, book count, digest, freshness,
and halt reason. `READY` requires a synchronized epoch, fresh market data, and
authoritative books. A crossed delta or best-price mismatch moves the affected
book into snapshot recovery without making it tradable.

```text
public event -> checksummed append -> bounded channel -> one writer
                                             \-> health/digest snapshots
```

The deterministic health core accepts explicit time. Only the runtime wrapper
reads the system clock.

### `reference-market-data`

Owns the read-only Binance Spot boundary for the BTCUSDT and ETHUSDT underlying
markets. It captures UTC one-hour candles, aggregate trades, and best bid/ask
updates from the official market-data-only combined WebSocket.

The types enforce an oracle boundary: only a finalized one-hour candle is
settlement-reference evidence. Open candles, aggregate trades, and book tickers
are predictive observations. Underlying prices use uncapped fixed-point
`QuotePriceMicros`, never the binary-outcome `PriceMicros` type.

```text
Binance combined stream -> strict typed normalization -> checksummed journal
                                              \-> bounded optional live channel
checksummed journal -> transactional replay -> health + immutable candles
```

The gateway uses verified WebPKI roots with explicit TLS 1.2 and HTTP/1.1 ALPN.
Reference quantities use a distinct exact 1e-8 fixed-point type because Binance
publishes up to eight decimal places; prices remain in micros and no implicit
conversion enters execution or accounting domains.

Aggregate-trade and book-ticker source IDs must increase but may have gaps.
Finalized candles cannot change. Book tickers explicitly carry unavailable
source event time rather than substituting receive time. Readiness requires all
three feed classes for both symbols within the active connection epoch.

### `resolution-rules`

Owns deterministic binding between immutable Polymarket hourly rules and the
exact finalized Binance candle. It validates the configured series and asset,
exact resolution URL, reviewed rule clauses, UTC hour alignment, Binance
symbol, and candle boundaries.

```text
MarketIdentity + reviewed rule language -> ResolutionContract
ResolutionContract + open candle        -> indicative close-if-now assessment
ResolutionContract + finalized candle   -> immutable ResolutionEvidence
```

Equality resolves to `Up`. Open candles cannot create final evidence. Evidence
has an explicit versioned encoding, market/rules identifiers, winning token,
fixed-point prices, BLAKE3 checksum, and stable digest. Decoded evidence must be
validated again against its immutable contract. This evidence is an oracle
calculation, not proof that Polymarket has confirmed or paid resolution.

### `feed-supervisor`

Owns the deterministic readiness gate across independently healthy Polymarket
and Binance states. It consumes immutable snapshots and caller-supplied time;
it has no network, clock, strategy, or execution capability.

```text
market actor snapshot ----\
                           -> freshness + skew + history integrity -> READY
reference replay snapshot-/                                      \-> NOT READY
```

Every required market, candle, aggregate-trade, and book-ticker timestamp has
an independent freshness check. Source-event lag/future skew and cross-feed
receive skew have explicit budgets. Exact limits are inclusive.

Ordinary unavailable/stale/skew states can recover. Local clock regression,
future receive time, epoch/sequence regression, internally inconsistent timing,
or digest equivocation permanently halt readiness. The supervisor derives its
own marker digests from feed state plus timing metadata, so timestamp changes
cannot hide behind an unchanged feed digest. Online and replayed observations
use the same state transition and stable digest.

### `market-session`

Owns the deterministic hourly lifecycle across immutable identity, exact
condition/token books, reference candles, resolution contracts, and supervised
feed readiness. Exact start/end boundaries select current and next BTC/ETH
sessions. Ended sessions remain present while final oracle evidence is delayed
and while the next hour becomes active.

Supervisor provenance must match the exact market/reference epoch, sequence,
and state digest in the frame. Missing outcome books, missing candles, or
degraded supervision cannot produce a ready session. Identity overlap, clock
regression, provenance mismatch, and oracle mismatch halt transactionally.

### `session-runtime`

Owns the durable read-only boundary around `market-session`. Registrations and
complete coordination frames use a versioned bounded canonical encoding inside
the existing checksummed segmented journal.

```text
validated identity / captured frame
              -> append -> device sync -> single-writer coordinator -> watch snapshot
                    \-> strict replay + durable-prefix checkpoint validation
```

The bounded Tokio ingress reports full or closed explicitly. Journal or sync
failure never mutates coordinator state and terminates the live instance.
Checkpoints are prefix attestations: recovery replays authoritative events and
verifies the coordinator digest at the checkpoint sequence before accepting
later records.

### `integration-daemon`

Owns deterministic orchestration across feed supervision and durable hourly
coordination. Each tick follows one ordering:

```text
immutable feed state -> cross-feed supervisor -> exact session frame
                                           -> journal-first session runtime
```

Its production adapter reads existing book/reference replay cores and never
invents missing state. The bounded soak mode generates consecutive BTC/ETH
hours, exact rollovers, final oracle evidence, checkpoints, and deterministic
reports. Scheduled faults are explicitly pre-supervision or post-supervision.
Recoverable availability faults degrade readiness; integrity faults halt.

### `accounting-ledger`

Owns the offline deterministic capital-state foundation. Every transaction is
a set of signed fixed-point postings that sum to zero separately for collateral
and for each exact condition/token asset.

```text
canonical command -> business validation -> balanced posting preflight
                  -> append -> device sync -> single-writer ledger mutation
                                      \-> strict replay + prefix checkpoint
```

Available and reserved cash/tokens, inventory cost, fees, revenue, cost of goods
sold, locked tokens, and locked cost are distinct accounts. Controlled accounts
cannot become negative. Reservations have immutable ownership and partial
confirmed consumption. Only confirmed buy, sell, split, merge, and redemption
commands can create inventory or spendable proceeds; matched or pending states
have no posting path.

Exact duplicate command IDs are no-ops. Conflicting reuse is an absorbing
integrity failure. Complete-pair P&L remains locked and inaccessible until a
confirmed merge recognizes collateral. The crate has no exchange, wallet,
signing, strategy, or order capability.

### `settlement-reconciliation`

Owns the read-only boundary among immutable local trade expectations,
CLOB-reported settlement, Phase 2.0 ledger state, and finalized blockchain
balances.

```text
local trade fact ------\
CLOB lifecycle --------+-> deterministic status checks ----\
Phase 2.0 ledger view -+------------------------------------+-> RECONCILED / PENDING / HALTED
finalized chain view --/-> history + exact asset equality --/
```

The CLOB lifecycle is `MATCHED -> MINED -> CONFIRMED`, with documented retry
paths through `RETRYING` and terminal `FAILED`. Matched, mined, and retrying
states are explicitly unspendable. Terminal state and trade economics are
immutable.

An atomic reconciliation frame compares total confirmed collateral and every
exact condition/token balance. Confirmed trades require their expected ledger
command; failed and non-terminal trades forbid it. A bounded inclusive grace
period allows ledger ingestion after confirmation. Expiry, premature posting,
asset mismatch, source regression/equivocation, impossible lifecycle movement,
or unknown trade produces an absorbing halt.

Commands use the existing append-and-device-sync-before-mutation boundary,
strict segmented replay, stable digest, and prefix checkpoints. The crate
contains no authenticated user feed, RPC client, wallet, repair, signer, or
order capability.

The current CLI provides offline soak and strict recovery. The Phase 1.5 public-
feed eligible-network gate passed on 2026-07-17. Any later external orchestration
still requires an explicitly authorized read-only deployment phase.

### `portfolio-risk`

Owns the offline, deterministic authority that may approve or reject a proposed
order. It consumes only a fresh, ready Phase 2.1 reconciliation gate and the
exact digest-bound Phase 2.0 categorized ledger view.

```text
reconciled ledger + resting orders + candidate + shocks + limits
       -> validate exact reservation backing and candidate capacity
       -> enumerate fills x outcomes x correlated shocks
       -> conservative wealth and exposure witnesses
       -> APPROVE or attributable NO_TRADE
```

Resting orders and the candidate each have zero, configured-partial, and full
fill states. The Cartesian product is bounded before evaluation; every binary
market outcome and configured group shock is included. Conservative integer
rounding, category-specific haircuts, operational reserves, capital floor, and
gross/condition/group exposure ceilings apply to every scenario.

Normal safety failures are immutable `NO_TRADE` audit decisions. Reconciliation
history regression or equivocation, conflicting command IDs, arithmetic
failure, and durable-integrity failure halt permanently. Commands are bounded,
canonical, journaled and device-synced before mutation, replayable, and
checkpoint-verifiable. The crate cannot sign or submit an order.

### `order-intent-policy`

Owns the offline boundary between a Phase 2.2 approval and any future isolated
signer. It verifies the exact order fingerprint, risk-decision digest and age,
one-time approval use, exchange mode, time-in-force, and inert signer-policy
constraints.

```text
exact risk APPROVE + mode observation + signer-policy frame + order intent
       -> expiry, replay, venue, contract, token and amount checks
       -> PERMIT / DENY audit decision
       -> delayed / live / cancel-authorized / terminal paper lifecycle
```

Only `NORMAL` and safe `POST_ONLY` mode can permit placement. Cancel requests
remain possible in explicit reduction-safe modes but are denied in unknown or
restart state and throughout a delayed order's uncancellable interval. Mode and
clock regression, equal-sequence equivocation, lifecycle impossibility,
arithmetic failure, or durable-integrity failure halts transactionally.

The signer-policy frame is data, not a signer. The crate has no credential,
private key, signature primitive, authenticated client, network transport, or
order-submission capability. Commands use the same canonical journal-first,
strict-replay, and checksummed-prefix checkpoint boundary as accounting,
reconciliation, and portfolio risk.

### `paper-execution`

Owns the deterministic simulated/replayed boundary after Phase 2.3 policy. It
accepts no ambient exchange state: placement/cancel permits and every paper
exchange observation are explicit canonical inputs with source and receive
times.

```text
exact policy PERMIT -> submitted / delayed / acknowledged / live
                                  -> partial/full match ----> TradeIntent handoff
                                  -> cancel pending --------> fill/cancel race
                                  -> unknown/rejected ------> explicit recovery/class
```

Unknown is non-terminal and remains exposure-bearing. Cancel pending does not
imply cancellation; partial or full matches can still win the race. Exchange
order identity is immutable once observed. Match deltas must reproduce exact
cumulative quantity, consideration, and fees, respect the original limit, and
emit exactly one immutable reconciliation handoff per fill.

Retry classifications never trigger an automatic retry. Sequence/time history,
identity, cumulative arithmetic, and lifecycle violations halt. The component
uses journal-before-mutation, device sync, strict replay, stable digests, and
checksummed prefix checkpoints. It has no HTTP/WebSocket client, credential,
signature, wallet, RPC, or live submission capability.

### `paper-trading-runtime`

Owns the complete offline Phase 2.0–2.4 paper path as one deterministic writer.
It contains exactly one accounting ledger, settlement reconciler, portfolio-risk
engine, intent-policy engine, and paper-execution engine.

```text
actual reconciled state -> risk -> exact reservation -> intent policy
       -> paper execution -> unique fill handoff -> confirmed ledger posting
       -> CLOB lifecycle observations -> finalized-chain reconciliation
```

The runtime rejects risk requests that do not equal its current reconciliation
gate and categorized ledger view. Placement requires the exact approved order to
own the exact required collateral or token reservation. Policy and execution
consume the runtime's authentic last decision for the unchanged subject.
Reconciliation intent registration is private to a unique accepted execution
handoff, and reconciliation frames must reproduce the runtime's actual ledger
view. Confirmed ledger buys/sells must consume that handoff's exact command ID
and economics. Active, unknown, fully matched, and unposted partial-fill orders
retain their reservations; only safe pre-submission or terminal unused backing
can be released.

Paper delayed, live, and terminal observations update intent-policy lifecycle in
the same transactional transition. Explicit one-shot faults before risk,
execution, and handoff support deterministic recovery tests; integrity faults
and every child/cross-boundary integrity failure halt the whole owner.

Canonical pipeline commands are append-and-device-sync before mutation, strictly
replayed, and optionally verified against checksummed prefix checkpoints. The
runtime has no strategy, credential, signer, authenticated transport, RPC,
wallet action, automatic retry, or live submission capability.

### `strategy-proposal`

Owns the deterministic proposal-only boundary between exact hourly-session
truth and Phase 2.2 portfolio risk.

```text
exact applied coordination frame + bounded fixed-point strategy intent
       -> provenance/readiness/book/economic gates
       -> inert OrderExposure candidate or attributable rejection
```

The captured context binds coordinator, frame, session, market, reference, and
supervision digests; the current session identity; and exact authoritative Up
and Down book views. Only the current `ACTIVE_READY` session can produce a
candidate. The boundary derives the risk-order identity, consumes each proposal
identity once, and treats history equivocation or integrity failure as an
absorbing halt.

Commands use bounded canonical encoding, journal-before-mutation and device
sync, strict segmented replay, stable digests, and checksummed prefix
checkpoints. The output is not a risk approval or an order. The crate has no
capital reservation, policy permit, signer, credential, network, wallet, or
submission capability and currently implements no strategy alpha.

### `complete-set-arbitrage`

Owns conservative offline detection of complementary-token top-of-book
opportunities from immutable Phase 2.6 context.

```text
ready strategy context + pair direction + explicit fee/conversion bounds
       -> conservative top-level quantity and integer pair economics
       -> exactly two inert proposal intents or attributable no-opportunity
```

Buy-pair legs round required collateral upward; sell-pair legs round proceeds
downward. The smaller selected book level caps quantity. Explicit maximum fees,
conversion cost, minimum net profit, and minimum ROI are applied before output.
Plan and proposal identities bind the exact context, direction, and constraints.

The result is not locked profit because the legs have not filled. It must still
pass proposal validation, all open-order fill permutations in portfolio risk,
reservations, policy, execution, confirmation, and reconciliation. The crate
has no split/merge, capital, risk, signer, credential, network, wallet, or order
capability. Commands are journal-first, replayable, and prefix-checkpointed.

### `paired-opportunity-runtime`

Owns one offline detector, proposal engine, and portfolio-risk engine so their
decision subjects cannot be substituted between Phase 2.7, 2.6, and 2.2.

```text
exact arbitrage command -> authentic opportunity -> two authentic candidates
       -> shared capacity + independent two-leg fill permutations
       -> paired risk-eligible / NO_TRADE audit decision
```

Portfolio risk accepts at most two unreserved candidates, aggregates their
available-cash or per-token inventory demand, and places each candidate's
zero/partial/full fills in the same scenario product. Existing resting orders
still require exact reserved backing.

The pair receives a candidate-set digest that deliberately cannot satisfy the
single-order fingerprint required by placement policy. The paper runtime rejects
multi-candidate approvals as an additional boundary. Thus paired risk eligibility
is not authority to place either leg. The runtime has no reservation, policy,
execution, split/merge, credential, signer, wallet, or network capability.
Commands are journal-first, replayable, and prefix-checkpointed.

### `paired-capital-staging`

Owns one paired-opportunity runtime and one accounting ledger. The exact current
ledger risk view is required at the Phase 2.8 boundary; detached balance claims
or paired decisions are not accepted.

```text
current ledger + exact paired evaluation -> risk eligible with two candidates
       -> reserve both on transactional clone -> fully reserved inert stage
       -> abort both before downstream authority, or retain both
```

Each buy reservation covers rounded full consideration and maximum fee; each
sell reservation covers exact token quantity. The runtime installs the clone
only after validating both reservations, so no observable one-leg state exists.
Abort releases both reservations together. Journal sync precedes mutation, and
replay/checkpoints reproduce ledger, paired-child, stage, and halt state.

The stage is capital evidence only. This crate has no placement policy,
execution sequencing, split/merge, credential, signer, wallet, network, or order
capability.

### `paired-placement-policy`

Owns Phase 2.9 staging and creates short-lived paper-only permissions from exact
fully reserved stages under a current explicit `NORMAL` exchange mode.

```text
fully reserved pair -> first-leg paper permit -> monotonic paper lifecycle
       -> first fully matched -> complementary hedge paper permit
       -> retain both reservations until provably safe paired abort
```

Permits bind full candidate economics, reservation identity, stage digest, leg
role, mode sequence, and a validity interval within the one-second stage window.
The original candidate expiry can shorten that interval further.
Expiry never releases capital. Submitted, delayed, live, partial, unknown,
matched, and hedge-active states preserve both reservations. Safe abort exists
only when both legs prove zero possible fill and delegates to the transactional
Phase 2.9 paired release.

The runtime contains no execution gateway, signature, credential, authenticated
transport, wallet, split/merge, or live order capability. Commands are
journal-first, replayable, and prefix-checkpointed.

### `paired-paper-execution`

Owns Phase 2.10 policy plus all paired simulated orders so lifecycle state
cannot be injected around the execution boundary.

```text
exact paired permit -> one-use paper submission -> monotonic exchange events
       -> partial/full fixed-point fills -> immutable settlement handoffs
       -> synchronized paired policy while both reservations remain locked
```

The kernel models delayed uncancellable windows, acknowledgements, live orders,
unknown outcomes, cancel-pending races, partial/full matches, cancellations, and
rejections. Unknown and cancel-pending orders remain fillable. Every fill is
bounded by the original candidate and produces one unique handoff; handoffs are
not posted or treated as confirmed inventory.

Caller policy lifecycle commands are rejected. The owner verifies the complete
ledger risk view is unchanged across every execution transition. It contains no
authenticated adapter, signer, credential, network submission, wallet,
split/merge, settlement confirmation, or accounting-posting capability.
Commands are journal-first, replayable, and prefix-checkpointed.

### `paired-settlement-runtime`

Owns Phase 2.11 execution and one Phase 2.1 reconciler so execution handoffs,
settlement lifecycle, confirmed accounting, finalized-chain comparison, pair
locking, and residual capital release cannot be reordered or substituted.

```text
stored execution handoff -> registered local intent -> CLOB settlement states
       -> exact CONFIRMED posting -> finalized-chain reconciliation
       -> optional complete-pair lock -> transactional paired finalization
```

Callers identify a stored handoff by stage, leg, and index; they cannot supply a
trade intent. Confirmed postings are derived from immutable handoff economics
and consume the nested stage reservation. Reconciliation frames use the ledger
nested under Phase 2.11 rather than accepting a claimed view. Matched, mined,
retrying, and failed trades have no successful posting path.

Residual reservations can leave active state only after both orders are
terminal, every handoff is registered, all trades are terminal, confirmed
trades are posted, failed trades are unposted, and the finalized reconciliation
covers the exact current ledger digest. The release batch handles both legs
transactionally. Equal confirmed complementary buy inventory can enter a ledger
pair lock, but locking does not imply a merge or spendable proceeds.

The top-level owner is journal-first, strictly replayable, digest-stable, and
prefix-checkpointed. It has no authenticated adapter, signer, credential, RPC,
wallet, split/merge transaction, automatic retry, or live order capability.

### `ctf-transaction-runtime`

Owns Phase 2.12 plus deterministic split, merge, and redemption records.

```text
current reconciled ledger -> reserve collateral/token or lock complete pair
       -> requested -> pending <-> retrying -> confirmed / failed
       -> confirmed-only accounting or exact failure policy
```

Split and redemption inputs enter ledger reservations at request time. Merge
inputs enter or bind a complete-pair lock. Pending and retrying assets are
therefore unavailable to risk or another conversion. Source sequence, event and
receive time, external transaction identity, and confirmation hash are explicit
immutable facts. Duplicate submission reports are attributable no-ops and no
retry observation performs an action automatically.

Confirmation derives one accounting command from the stored request: split
creates equal complementary quantity with combined cost equal to consumed
collateral; merge consumes one active lock; redemption consumes reserved tokens
for a bounded payout tied to a nonzero resolution digest. Failed split or
redemption releases its reservation, while failed merge retains its lock for an
explicit recovery request.

The owner is journal-first, replayable, prefix-checkpointed, and fail-closed. It
contains no RPC, wallet, relayer, allowance mutation, signer, credential,
authenticated transport, automatic retry, or live transaction capability.

### `unified-paired-trading-runtime`

Owns Phase 2.13 and is the public orchestration boundary for the complete
offline paired path.

```text
market/risk inputs -> internally derived evaluation provenance -> pair stage
    -> authorize + submit atomically -> simulated fills -> stored handoffs
    -> confirmed settlement/accounting -> pair lock -> CTF finality
```

Its command language contains domain inputs and simulated observations, not
generic child commands. Current reconciliation and nested-ledger risk
provenance are derived internally. Child IDs, issued permits, paper-fill ledger
IDs, and confirmed-posting subjects are also derived or selected from owned
state. Authorization and submission install together or not at all.

The owner journals only top-level commands and checkpoints one complete nested
digest. Deterministic boundary faults, pending-conversion restart recovery,
full paired settlement/merge composition, 24-hour no-trade profiles, and
multi-hour split/merge capital-conservation profiles are exercised offline.
It has no credential, signer, authenticated transport, RPC, wallet, relayer,
automatic retry, or live order/transaction capability.

### `shadow-adapter-certification`

Owns deterministic pre-authentication adapter evidence and certification.

```text
immutable interface contract + recorded fixtures + policy dry runs
    + region eligibility + synthetic allowance/gas/relayer observations
    + mandatory failures -> CERTIFIED / NOT_CERTIFIED audit report
```

The contract binds venue, public hosts, chain/contracts, schema, regions,
freshness and operational limits. Recorded fixture kinds map to fixed safe
responses for restart, mode restriction, delay, tick changes, rate limits,
unknown orders, settlement retries, and heartbeat loss. Signer dry runs inspect
policy/intent fields but cannot access a key or produce a signature.

Certification requires complete happy-path and denial evidence, every planned
region to be independently eligible and fresh, healthy synthetic operational
state, and all mandatory adverse scenarios. Every report explicitly grants no
authority. The component has no network, RPC, credential, signer, wallet,
relayer-client, deployment, order, or transaction capability.

### `shadow-gateway-harness`

Owns the credentialless Phase 2.16 composition of Phase 2.15 evidence with the
Phase 2.14 unified offline runtime.

```text
fresh non-authorizing certification + complete-stack heartbeat
    + recorded fixture / explicit recovery evidence
    -> derived simulated exchange mode -> gated Phase 2.14 domain command
    -> top-level journal + complete nested-digest checkpoint
```

The harness independently checks certification and heartbeat age at every
exposure boundary. Certification expiry and heartbeat failure derive a
simulated `TRADING_DISABLED` observation before further exposure. The dead-man
state retains backing because neither a cancel request nor a lost heartbeat
proves exchange cancellation.

Restart recovery requires current reconciliation and explicit unknown-order
clearance, then derives `RECOVERING` followed by `NORMAL`. Callers cannot inject
mode observations. Recorded restart, post-only, cancel-only, taker-delay,
tick-change, rate-limit, unknown-order, settlement-retry, and heartbeat-loss
fixtures translate to conservative simulated observations without automatic
retry or capital release.

The owner is journal-first, strictly replayable, digest-stable, and prefix-
checkpointed. It adds no network dependency or client and has no credential,
signer, authenticated transport, RPC, wallet, relayer client, deployment, or
live order/transaction capability. Existing public read-only market-data
dependencies remain below the Phase 2.14 domain type graph but are never called
by this harness.

### `shadow-session-campaign`

Owns bounded multi-day recorded-session and fault campaigns over one Phase 2.16
gateway.

```text
sealed manifest + hash-chained scheduled steps + recorded session digests
    -> exact active-session replay through Phase 2.16
    -> outcome-derived fault coverage + terminal readiness/backing checks
    -> checksummed non-authorizing operator evidence bundle
```

The manifest commits to the complete session set, duration, required scenarios,
step count and final step-chain digest. Runtime replay cannot occur outside its
active session. Certification, heartbeat, fixture, expiry and recovery controls
may execute between sessions, but their coverage is recognized only from the
actual accepted gateway outcome.

Finalization is attributable even when the campaign is incomplete. Evidence is
eligible for operator review only after the exact schedule, all sessions and
all required scenarios complete, the nested gateway finishes ready, reserved
cash is zero and no conversion remains pending. A positive bundle still sets
operator-decision-required and grants no promotion or deployment authority.

Commands are journal-first and complete-state checkpointed. Evidence files are
create-new, bounded, canonical, versioned, BLAKE3-checksummed and internally
digest-verified. The crate adds no network client and has no credential,
signer, authenticated transport, RPC, wallet, relayer, automatic retry,
promotion, deployment or live submission capability.

### `promotion-governance`

Owns deterministic offline release-candidate governance above Phase 2.17.

```text
canonical independent campaign bundles + sealed regression baseline
    + source/binary/toolchain/lock/SBOM/config digests + rollback criteria
    -> diversity and upward-rounded regression gates
    -> distinct risk/release operator decisions on the exact subject
    -> checksummed non-deploying canary-eligibility record
```

Only fresh eligible campaign bundles contribute to aggregate campaign, session,
step and fault totals. Duplicate bundle or campaign identities cannot inflate
those totals. Manifest, schedule and final-gateway-state diversity are counted
independently. Regression floors use checked basis-point multiplication and
round upward against the sealed baseline.

The candidate binds source, binary, toolchain, dependency-lock, SBOM and
configuration digests plus explicit rollback timing, loss, fault, capital-floor
and reconciliation criteria. Risk and release decisions must bind that same
candidate and come from distinct opaque operator identities. These identifiers
provide deterministic accountability only; they are not credentials or
cryptographic authentication.

Finalization always emits attributable evidence. `CANARY_ELIGIBLE` still
requires future operator execution and sets canary execution, promotion,
deployment, credential and live-trading authority to false. Commands are
journal-first and prefix-checkpointed; record files are create-new, canonical,
bounded, versioned, BLAKE3-checksummed and internally digest-verified. The crate
adds no network dependency and cannot build, upload, deploy, roll back, sign,
authenticate, access a wallet/RPC endpoint, or submit anything live.

### `canary-rollout-simulator`

Owns one deterministic offline rollout and abort-controller simulation above an
exact Phase 2.18 record.

```text
unexpired canary eligibility + matching rollback criteria + sealed plan
    -> current health and maintenance-window gate
    -> ordered simulated stages / explicit pause and resume
    -> abort, latched rollback or simulated completion
    -> checksummed non-executing rollout report
```

The plan contains ordered non-overlapping maintenance windows and strictly
increasing target basis points with minimum observation and maximum stage
durations. Health frames independently preserve strategy, risk, market feed,
user feed, reconciliation, capital-floor, unresolved-age, unknown-age, loss and
fault state with exact sequence, timestamps and provenance.

Ordinary health loss or stale health pauses a running simulation; healthy data
does not resume it. Capital-floor breach, threshold excess, stage timeout or
plan timeout irreversibly latches simulated rollback requirement. Restart keeps
the exact stage, requires post-restart health and a new recovery epoch, and
returns paused for explicit operator review.

Commands are journal-first and prefix-checkpointed. Reports are create-new,
canonical, bounded, versioned, BLAKE3-checksummed and internally digest-verified.
All operator identifiers are opaque accountability data. The crate adds no
network dependency and cannot route traffic, allocate capital, deploy, execute
rollback, authenticate, sign, access RPC/wallet state, or submit anything live.

### `fleet-rollout-governance`

Owns deterministic offline fleet evidence and release revocation above exact
Phase 2.19 reports.

```text
region-bound completion + abort drills + rollback drills and trigger diversity
    + exact release/artifact/rollback binding + sealed change freeze
    -> canonical deduplicated fleet aggregate + irreversible revocation state
    -> checksummed non-deploying operational-readiness dossier
```

Every required region needs an independent simulated-completion report. Abort
and rollback drill floors are distinct, and configured rollback triggers must
all be covered. Duplicate report or rollout-plan digests are excluded before
coverage is counted; stale reports are attributable but do not pass a gate.

The change freeze binds the exact release and artifacts, is start-inclusive and
end-exclusive, and encloses the campaign validity interval. Revocation binds
the same subject plus an opaque operator and reason, is irreversible, and
denies readiness even if all regional evidence passes. It remains available
after a positive dossier, immediately makes that dossier historical, and
requires a superseding non-ready dossier.

Commands are journal-first and prefix-checkpointed. Dossiers are create-new,
canonical, bounded, versioned, BLAKE3-checksummed and internally digest-verified.
The crate adds no network or cloud dependency and cannot route, deploy, execute
rollback, authenticate, sign, access credentials/RPC/wallet state, allocate
capital, or submit anything live.

### `deployment-preflight`

Owns deterministic offline package and multi-operator preflight above a current
unrevoked Phase 2.20 readiness binding.

```text
current fleet binding + exact regional deployment configurations
    + credentialless least-privilege limits + tested rollback package
    + distinct release/risk/operations decisions
    -> checksummed non-deploying manual-preflight report
```

The fleet owner derives a current binding only while its positive dossier is
still installed and no revocation exists. Package registration requires exact
region equality and nonzero image, configuration, infrastructure, network,
observability and failover provenance. Public administration, embedded secret
material, arbitrary transfer, withdrawal and upgrade privileges are rejected.
Notional and loss limits cannot exceed the sealed preflight policy.

Rollback binary, configuration, runbook and verification evidence bind the
unchanged release subject. Release, risk and operations decisions are fresh,
role-bound and require distinct opaque operators. Finalization supplies a
renewed fleet binding, so changed or revoked governance cannot reuse an old
readiness artifact.

Commands are journal-first and prefix-checkpointed. Reports are create-new,
canonical, bounded, versioned, BLAKE3-checksummed and internally digest-verified.
The crate has no network or cloud client and cannot create credentials, sign,
deploy, route, execute rollback, access RPC/wallet state or trade live.

### `deployment-orchestration-simulator`

Owns deterministic offline regional wave and rollback orchestration above one
exact current Phase 2.21 report.

```text
current preflight + exact regions + sealed ordered waves
    -> contiguous independent regional health gates
    -> start / advance / explicit pause-resume / restart recovery
    -> irreversible reverse-order rollback convergence or completion
    -> checksummed non-executing orchestration report
```

Every preflight region occurs exactly once across unique ordered waves. Start,
advance and resume require fresh package, service, risk, reconciliation and
capital-floor health for the relevant scope. Ordinary degradation pauses;
reconciliation failure, capital-floor breach, timeout or post-activation abort
latches rollback. Healthy evidence never clears a pause or rollback latch.

Rollback observations bind the exact rollback package and converge activated
regions exactly once in reverse order. Restart retains progress, requires a new
evidence epoch, and returns non-rollback work paused. Commands are journal-first
and prefix-checkpointed; reports are create-new, canonical, bounded, versioned,
BLAKE3-checksummed and digest-verified. The crate has no network or control-plane
client and cannot create credentials, deploy, route, execute rollback, access
RPC/wallet state, allocate capital or trade live.

### `deployment-adapter-certification`

Owns deterministic offline deployment-adapter and disaster-recovery evidence
above exact Phase 2.22 completion and rollback reports.

```text
exact completion + reverse rollback + credentialless adapter contract
    -> complete contiguous fixture matrix in every region
    -> baseline privilege data test + mandatory privilege denials
    -> bounded regional/control-plane/state/artifact recovery drills
    -> checksummed non-authorizing adapter certificate
```

Every region records discovery, dry-run, apply-plan, health, traffic-plan,
rollback-plan, partition, rate-limit, authentication-denial and unknown-state
behavior in a fixed sequence. Outcomes only observe, deny, require manual
execution, back off or require reconciliation. Mutation and credential claims
halt rather than contributing evidence.

The privilege policy permits only exact resource-bound read and planning data;
wildcards, secrets, cluster administration, arbitrary execution, escalation and
cross-region mutation must be denied. Recovery drills prove journal/checkpoint
reconstruction, reconciliation, rollback availability and manual failover for
all regions. Commands are journal-first and prefix-checkpointed; reports are
create-new, canonical, bounded, versioned, BLAKE3-checksummed and digest-
verified. The crate has no network, cloud SDK or Kubernetes dependency and
cannot authenticate, deploy, route, fail over, execute rollback or trade live.

### `deployment-change-control`

Owns deterministic offline change-control sequencing above one exact current
Phase 2.23 adapter certificate.

```text
current certificate + exact regions/package subjects + sealed plan
    -> distinct current release/risk decisions + active maintenance window
    -> expiring one-use next-step manual handoff
    -> pause invalidation / safe pre-handoff abort
    -> irreversible post-handoff reverse rollback convergence
    -> checksummed non-authorizing change-control report
```

The plan contains unique ordered windows and contiguous region-bound change
steps. A permission expires at the earliest plan, window or policy boundary,
binds exactly one step and can be consumed once. Consumption records only that
a manual handoff was simulated; it does not claim that infrastructure changed.
Pause destroys an outstanding permission. Resume requires current dual control
and a current window and can never revive the old permission.

Abort before any handoff is a safe terminal result. Abort or a configured
severe signal afterward irreversibly requires one-use rollback handoffs for
consumed steps in exact reverse order. Commands are journal-first and prefix-
checkpointed; reports are create-new, canonical, bounded, versioned,
BLAKE3-checksummed and digest-verified. The crate has no credentials, network,
cloud SDK or Kubernetes client and cannot authenticate, deploy, route, execute
rollback, access RPC/wallet state or trade live.

### `deployment-change-campaign`

Owns deterministic offline operational campaigns over independent sealed Phase
2.24 plans.

```text
sealed independent plans + exact child command schedules + expected terminals
    -> fresh authentic Phase 2.24 owner per case
    -> multi-window / approval-expiry / pause / abort / rollback drills
    -> exact-prefix restart reconstruction and digest equality
    -> ordered case and result chains
    -> checksummed non-authorizing operator evidence
```

Manifest registration validates every bounded child schedule before installing
state. Cases then run exactly once in manifest order. Approval renewal requires
fresh dual-control sets across independent plans; expiry coverage requires an
authentic child denial. Safe abort and emergency rollback are distinguished by
consumed work, trigger and reverse-convergence evidence. Coverage cannot be
claimed by callers.

Commands are journal-first and prefix-checkpointed. Evidence is create-new,
canonical, bounded, versioned, BLAKE3-checksummed and digest-verified. Even an
eligible record requires a future operator decision. The crate has no network,
credentials, cloud SDK or Kubernetes client and cannot authenticate, deploy,
route, execute rollback, access RPC/wallet state or trade live.

### `production-change-readiness`

Owns deterministic offline governance above canonical Phase 2.25 campaign
evidence.

```text
fresh independent campaign evidence + exact downstream subject union
    + sealed regression baseline + release/binary/config/infrastructure binding
    -> canonical deduplication + freshness + five diversity/regression gates
    -> distinct release/risk/operations decisions on the unchanged subject
    -> checksummed non-executable production-change readiness record
```

Only fresh eligible campaigns contribute. Duplicate evidence and campaign
identities cannot inflate totals; independent-plan and approval-set totals are
bounded by unique plan subjects. Manifest, schedule, result-chain and plan
diversity remain independent. Checked basis-point floors round upward for
campaign, case, plan, restart and approval evidence.

The three opaque operators provide deterministic accountability, not identity
authentication. Commands are journal-first and prefix-checkpointed; records are
create-new, canonical, bounded, versioned, BLAKE3-checksummed and digest-
verified. A ready record requires future operator execution and grants no
credential, deployment, rollback, traffic, cloud-control or live authority.
The crate contains no network, cloud SDK, Kubernetes, RPC or wallet client.

### `deployment-execution-intent`

Owns the credentialless Phase 2.27 boundary between a sealed production-change
readiness record and any future executor integration.

```text
current Phase 2.26 record + exact subject + least-privilege contract
    -> fixed credential/signing/transport/replay dry-run matrix
    -> certified next-step-only one-use manual handoff intents
    -> checksummed non-executing completion evidence
```

Regions, operations and resource digests are exact canonical allowlists;
wildcards and privilege escalation have no valid representation. Only one
short-lived next-step intent can exist, and consumption records an opaque manual
handoff rather than performing it. Commands are journal-first and strictly
replayable. The crate contains no secrets, signer, network client, cloud SDK,
Kubernetes client or external-submission path.

### `executor-session-simulator`

Owns the credentialless Phase 2.28 protocol boundary above one exact completed
Phase 2.27 report and plan.

```text
current Phase 2.27 evidence + exact isolation/request contract
    -> exclusive expiring lease + isolated simulated session
    -> ordered request envelopes + acknowledged/rejected/unknown fixtures
    -> dead-man/restart no-mutation reconciliation
    -> checksummed non-executing session dossier
```

One request can be active and only the next exact Phase 2.27 step template may
be issued. Unknown outcomes, unhealthy heartbeats, lease expiry and restart
disable new work; uncertainty is cleared only by digest-bound no-mutation
reconciliation. A simulated acknowledgement never claims an external
acknowledgement or deployment. The crate contains no sockets, credentials,
signer, cloud SDK, Kubernetes client or submission path.

### `transport-adapter-certification`

Owns the Phase 2.29 recorded-fixture boundary above a current exact Phase 2.28
session dossier.

```text
current session dossier + exact template digests
    + HTTPS/SNI/SPKI/path policy + canonical request bytes
    -> fixed DNS/TLS/endpoint/serialization fixture matrix
    -> timeout/rate-limit backoff + unknown-response reconciliation
    -> checksummed non-networking transport certificate
```

Both positive and negative fixtures validate recorded fields, not caller labels.
Timeout and rate-limit evidence never retries; unknown response blocks
certification until the next exact no-mutation reconciliation fixture. The crate
contains no DNS resolver, socket, TLS engine, HTTP client, credential, signer or
external submission path.

### `credential-broker-simulator`

Owns the Phase 2.30 zero-key signing-policy boundary above one current exact
Phase 2.29 transport certificate.

```text
current transport certificate + opaque attested handle + exact policy
    -> fixed signer success/denial/failure fixture matrix
    -> distinct security/operations authorization
    -> short-lived one-use permit -> digest-only simulated receipt
```

Purposes, subject digests, integer units, campaign bounds and unique nonces are
validated before registration. Approval freshness is rechecked when a permit
is issued. The state machine alone creates receipts and irreversible revocation
clears an active permit. Commands are journal-first and strictly replayable;
reports and checkpoints are create-new and checksummed. The crate contains no
private or public key bytes, signature implementation, KMS/HSM/Vault client,
credential, socket, authenticated transport or external-submission path.

### `submission-gateway-certification`

Owns the Phase 2.31 recorded authenticated-envelope and exactly-once shadow
submission boundary above exact Phase 2.29 and Phase 2.30 evidence.

```text
transport plan/certificate + broker plan/report + exact receipt chain
    -> endpoint/request/channel/token-bound shadow envelopes
    -> fixed success/denial/backoff/ambiguity fixture matrix
    -> one exactly-once shadow submission -> accept/reject/unknown
    -> exact no-mutation reconciliation for unknown
```

Every envelope maps one broker request and receipt to the same contiguous
transport request binding. Staging consumes receipt and idempotency identities
once but performs no I/O. Unknown remains active and prevents later work until
recorded no-mutation evidence matches its exact observation. There is no retry
transition. Commands are journal-first and replayable; reports and checkpoints
are create-new and checksummed. The crate contains no secret value, key,
signature engine, authorization-header value, resolver, socket, HTTP client or
external-submission path.

### `shadow-auth-session`

Owns the Phase 2.32 exclusive offline session lifecycle over one current exact
Phase 2.31 certification report.

```text
current gateway certification + recorded attestation
    -> exclusive bounded opaque-owner lease + monotonic heartbeats
    -> clean close / predecessor-bound rotation
    -> dead-man | unhealthy | restart | ambiguity revocation
    -> exact recorded no-mutation recovery -> idle, never auto-open
```

Lease expiry is bounded by policy, plan, gateway freshness and attestation.
Rotation preserves gateway/channel/token subjects and requires the exact prior
attestation digest. Every disruption revokes before recovery; recovery is a
separate transition and cannot restore a lease. Commands are journal-first and
replayable; reports and checkpoints are create-new and checksummed. The crate
contains no credential, certificate private key, signature, provider client,
socket, authenticated transport or external-submission path.

### `credential-provider-certification`

Owns the Phase 2.33 offline certification protocol over one exact current Phase
2.32 shadow-session report.

```text
current shadow-session report + inert provider contract
    -> opaque acquisition -> predecessor-bound rotation
    -> quota/outage backoff + mismatch/stale denial
    -> split-brain revocation -> exact inactive recovery
    -> explicit revocation + complete fixed scenario certificate
```

Provider handles contain digests and bounded epochs only. Revocation is
irreversible; split-brain revokes before recovery; disaster recovery proves
continuity in a distinct configured region but never activates a handle.
Commands are journal-first and replayable; reports and checkpoints are
create-new and checksummed. The crate contains no credential, key material,
signature, provider client, socket, wallet, deployment or order capability.

### `durable-infrastructure`

Owns Phase 3.0 credentialless contracts and local certification for PostgreSQL,
Redpanda, ClickHouse and Parquet-compatible archives.

```text
Phase 2.33 evidence + four immutable backend contracts
    -> chained commit/idempotency/ordering fixtures
    -> backpressure/corruption/migration/rollback fixtures
    -> exact restore and replay convergence
    -> local certificate with zero environment authority
```

PostgreSQL can preserve exact ledger projections; it cannot originate facts.
Redpanda distributes ordered events, ClickHouse stores derived analytics and
Parquet-compatible storage archives replay evidence. Every command is
journal-first and recoverable. No adapter in this phase opens a socket or
contains credentials.

### `security-boundary`

Owns Phase 3.1 local certification of workload identity, fake Vault/KMS/HSM
providers and the isolated signer policy.

```text
current Phase 3.0 report + identity/provider/signer contracts
    -> opaque issue -> predecessor-bound rotation
    -> outage/rate/denial/replay/dual-control fixtures
    -> compromise revocation -> exact inactive recovery
    -> explicit revocation + local no-authority certificate
```

Strategies cannot reach this boundary directly. Purpose, resource, integer
notional, rate, expiry and dual-control limits are independently enforced.
Compromise revokes before recovery and recovery never activates identity or
signing. The phase uses fake providers and opens no socket.

### `read-only-venue`

Owns Phase 3.2 deterministic supervision of public-market, authenticated-user
observation, REST metadata and reference-price channels.

```text
current Phase 3.1 report + subscription-only user contract
    -> four independent channel epochs and freshness gates
    -> fixed-point versioned parameters + explicit exchange mode
    -> restart/channel failure invalidation
    -> complete fresh snapshots + no-mutation recovery
```

The authenticated contract contains no credential or mutation endpoint. A
healthy channel cannot hide another channel's failure, and cached state never
crosses reconnect epochs. Local certification grants no live-environment,
authentication, execution or deployment authority.

### `chain-observer`

Owns Phase 3.3 deterministic, read-only blockchain and wallet truth.

```text
current Phase 3.2 report + exact chain/wallet subjects
    -> three independent read-only provider snapshots
    -> exact finalized-block and wallet-state agreement
    -> pre-finality reorg invalidation + complete fresh recovery
```

Collateral, allowance, CTF balances and transaction lifecycle remain separate
fixed-point observations. Pending or merely mined effects are never promoted to
finalized spendable state. Provider staleness is independently bounded;
disagreement, chain substitution and finalized equivocation fail closed. The
crate contains no RPC transport, credential, signer, wallet or mutation path.

### `continuous-shadow-certification`

Owns Phase 3.4 deterministic accelerated campaign evidence over the complete
read-only observation boundary.

```text
current Phase 3.3 report + immutable runtime/config subjects
    -> contiguous healthy ticks + independent resource budgets
    -> rollover/restart/partition/dead-man drills and recovery
    -> dual-operator local report with real-soak=false
```

Logical campaign duration remains separate from real elapsed time. Every
disruption invalidates readiness before recovery, while integrity failures are
tested only in isolated halt-class fixtures. This layer opens no connection and
cannot convert local campaign evidence into environment or execution authority.

### `live-data-paper-certification`

Owns Phase 3.5 point-in-time paper evidence from immutable captured sessions.

```text
current Phase 3.4 report + capture manifest + chronological folds
    -> available-time gate + three queue cases + bounded latency
    -> complete paper outcomes + authentic downstream digest chain
    -> frozen-test local report with real-pnl=false
```

Source event, receive and strategy-available times remain separate. Passive
fills require queue evidence rather than price touch. The layer records existing
paper authority outputs but cannot inject a fill, posting, balance or mutation.

### `authenticated-no-submit`

Owns Phase 3.6 local certification of opaque authenticated-observation identity
while submission remains independently absent and denied.

```text
current Phase 3.5 report + observation-only contract
    -> opaque issue/rotation + recorded fixture matrix
    -> outage/dead-man/unknown/DR reconciliation + revocation
    -> local report with identity/connection/submit=false
```

The code contains no credential or signature implementation and opens no
connection. Physical endpoint absence and logical mutation denial are separate
mandatory proofs; local evidence cannot activate a real production identity.

### `micro-capital-canary-controller`

Owns Phase 3.7 code-only canary control certification.

```text
current Phase 3.6 report + exact allowlist + tiny fixed limits
    -> dual control + complete-set/no-trade/denial cases
    -> kill/dead-man/abort/rollback simulation
    -> code-eligible report with capital/live-complete=false
```

All financial gates use signed checked fixed-point integers. No wallet, capital,
signer, transport or order exists in the crate; live canary evidence remains an
external authorization gate.

### `controlled-production-release`

Owns Phase 3.8 code-only production-release certification.

```text
current Phase 3.7 report + immutable release subjects + staged fixed ceilings
    -> three-person control + independently current regional health
    -> no-trade/expiry/reconciliation/incident/DR/rollback/revocation matrix
    -> revocable code-eligible report with production/live authority=false
```

Region health is sequence-bound and independently rechecked for freshness at
finalization. Capital stages use signed checked fixed-point integers and are
strictly increasing, but allocate no capital. The crate has no credential,
signer, wallet, transport, deployment or order path. Real target-environment
evidence and explicit external authorization remain outside this certificate.

### `terminal-projection`

Owns the Phase 4.0 public-data-to-operator-display boundary.

```text
Gamma identity + exact CLOB books + Binance current/hour-open reference
    -> bounded decode + fixed-point/identity/time/freshness validation
    -> atomic BTC+ETH schema-v1 snapshot
    -> local Bloomberg-style read-only terminal
```

The gateway is a single projection owner and binds only to loopback. A refresh
is published only when both assets and both complementary books pass together.
All other outcomes publish an empty discovering, stale or halted snapshot with
`NO_TRADE`. The browser validates the no-authority flags and applies an
independent freshness timeout. It never substitutes demo values. This projection
does not contain ledger, risk, wallet, user-channel, signer or order authority.

### `shadow-ops`

Owns deterministic operational safety around the read-only stack. Platform
adapters provide explicit runtime, progress, queue, memory, file, journal, and
latency samples. The core performs no system calls or sleeps.

```text
runtime snapshot + resource sample -> watchdog + integrity + budget gates
                                  -> ready / degraded / draining / stopped / halted
                                  -> stable OpenMetrics + audit digest
```

Resource excess is recoverable degradation. Clock/sequence regression,
impossible ingress state, invalid progress, watchdog expiry, or underlying halt
is an absorbing integrity failure. Drain prevents return to ready and stop is
ordered after drain. Named smoke, day, and seven-day profiles provide bounded
capacity evidence without adding Prometheus, Kubernetes, or paid services.

## Future boundaries

The following are architectural boundaries, not Phase 0/1 deliverables:

- feed gateways normalize untrusted external data;
- market actors own market state;
- portfolio risk allocates bounded budgets;
- strategies propose intents but cannot execute;
- an isolated signer enforces policy;
- reconciliation compares local, CLOB, blockchain, and ledger realities;
- PostgreSQL may later serve durable query projections; it does not replace the
  deterministic ledger transition and journal-replay authority;
- research and analytics remain outside the hot path.

Future predictive sources must remain separate from settlement-reference data
and use the same journal, replay, freshness, and health semantics.

## Time semantics

Every event records:

- `event_time_ns`: time asserted by the originating source, when available;
- `received_time_ns`: local monotonic-wall-clock capture timestamp;
- `sequence`: source-scoped ordering value assigned or normalized by the feed.

No component may substitute receive time for event time silently.
