# Changelog

All notable changes to this project are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/). This is a proprietary,
never-published workspace, so entries track milestones and repository changes
rather than released package versions.

## [Unreleased]

### Added
- Spec 096 Phase 4: all eight window setups are digest-bound under shared
  `SetupGateParams` (compiled defaults ≡ pre-policy hardcodes). Strategy Builder
  selects any plugin; API merges the full bag; Phase-2-only files default the
  remaining setups and migrate the active digest on load.
- Spec 096 Phase 3: schema-driven **Strategy Builder** workspace in the terminal
  (`STRATEGIES` / F9). Forms render from plugin parameter schema; validate /
  save version / clone / activate (Rust blake3) / audit tape — no per-setup
  React forms. Paper-only; live authority remains false.
- Spec 096 / ADR 0100: Window Policy Control Plane vertical slice. Operational
  parameters for window setups load from digest-bound versions under
  `var/strategy-control/` (compiled defaults = pre-096 hardcodes). Decisions
  journal `configuration_digest`; opens snapshot it. Minimal API at
  `/api/strategy-control` plus `btc-window-policy` CLI for activate. Paper-only;
  corrupt active policy fails closed at boot.
- Spec 094 / ADR 0098: laptop-local **Poly Advisor MCP** (`advisor/`) — read-only
  research cockpit for Claude/ChatGPT/Cursor. Live + journal data tools,
  1-second snapshot daemon, analytics/what-ifs, pattern + suggestion stores,
  spec drafting, ops/reports, and hard paper-only guardrails. Never places or
  signs orders; writes only under `var/poly-advisor/`.
- Spec 093 / ADR 0097: adaptive paper stake follows each setup's day win rate
  (weak 1.5% · unproven 2% · average 3% · good 6% · strong 10% of tradable
  bankroll; shadow capped at 3%). Shared $1,000 principal books a paper withdraw
  every time net realized reaches +50% ($500), then continues sizing on the
  reduced tradable bankroll. Positions GUI shows bankroll, withdraw progress,
  and the stake policy.
- Spec 089 / ADR 0093: fixed-point `btc-window-strategy` crate for every-second
  BTC 5m/15m/1h paper evaluation (`btc_window_engine_v1`), with universal
  exits/hedges/reversals, complete-set and sequential-lock scanners, and
  separated ACTIVE_CORE / SHADOW_EXPERIMENT journals. Legacy directional and
  fixed-threshold Python triggers are DISABLED as order triggers.
- Spec 090 / ADR 0094: managed `btc-window-paper` worker polls `/api/windows`,
  journals decisions under `var/btc-window-paper/`, and is supervised by
  `scripts/run-terminal-stack.sh` without Paper Week capital or live authority.
- Spec 091 / ADR 0095: the window engine evaluates every configured asset
  (BTC and ETH) and keeps a marked paper book. Positions carry stake, are
  valued at the executable bid for their own side, realize P&L on partial
  reductions and exits, and settle to a binary `1.0` / `0.0` payout with a
  `win` / `loss` outcome sourced from the observed reference level. A runtime
  ledger attributes results by setup, asset, and window while keeping
  ACTIVE_CORE and SHADOW_EXPERIMENT totals separate, and the terminal Positions
  workspace renders it as a P&L ribbon, asset books, open positions, a strategy
  leaderboard, and a filterable decision tape. Journal records written before
  this spec still deserialize.
- Spec 092 / ADR 0096: the paper book is durable. The runtime replays the day's
  decision journal into the ledger at boot, so realized P&L, win/loss counts and
  per-setup attribution are day-scoped instead of resetting to zero on every
  restart. Open positions are checkpointed atomically to
  `var/btc-window-paper/positions.json` with the risk state, recorded window
  open, and last observed snapshot, so a restart resumes a live position and
  settles one whose window ended while the worker was down. Recovery is
  all-or-nothing: an unparseable journal line, a corrupt checkpoint, or a
  checkpoint from another `strategy_version` halts startup with a named reason.
  Terminal records predating Spec 091 carry no realized P&L and are counted as
  skipped rather than booked as zero-P&L trades. `scripts/detach.py` starts a
  worker in its own session, because macOS has no `setsid(1)` and the launching
  shell was killing supervised workers.

### Fixed
- Journal and status write failures were discarded via `let _ = …`, so a failing
  disk was indistinguishable from a quiet market. Write errors are now collected
  in `journal_errors`, surfaced in the API as `HALTED_JOURNAL_NOT_DURABLE`, and
  stop the worker rather than let it keep trading against an incomplete journal.
- Two `btc-window-paper` workers could run concurrently against one journal
  directory. Both appended decisions and the position checkpoint became
  last-writer-wins, so a worker holding a funded LOCKED position had it erased by
  the other's empty checkpoint and abandoned with no exit or settlement record —
  observed on a real ETH 1h position. A worker now takes a single-writer pid lock
  (`writer.lock`) over the journal directory and refuses to start while a live
  worker owns it; a dead owner's lock is treated as stale so a crash cannot wedge
  the book.
- `EngineState::settle` took the position out of the engine before deciding it
  had no shares to settle, discarding it. It now checks before taking, so
  declining to settle can never drop a position that still holds quantity.
- `PaperRuntime` retired a departed condition even when no observed snapshot was
  available to settle against, silently discarding a funded position. It now
  keeps the engine and reports an `unsettleable` durability error instead.
- Observability: provisioned Grafana dashboard (`Poly Terminal — Read-Only
  Projection`) and pinned Prometheus datasource UID; mounted the dashboards
  directory in `deploy/docker-compose.yml`. Verified the Prometheus config and
  alert rules with `promtool` and confirmed Grafana provisions cleanly. Alert
  metric names match the gateway's real `/metrics` exports.
- Repository standards: `LICENSE` (proprietary), `SECURITY.md`,
  `CONTRIBUTING.md`, `CODEOWNERS`, PR template, `.editorconfig`, `CHANGELOG.md`.
- `Makefile` giving local/CI parity (`make verify`, `make ci`, `make tla`, …).
- Dependabot for cargo, npm (terminal), and GitHub Actions.
- CI: `cargo deny` job, terminal lint/build/test job, Rust build caching, and
  run-cancelling `concurrency`.
- `docs/STATUS.md`: scannable phase-gate and per-crate status summary.
- `docs/EDGE_ANALYSIS.md` and research tooling under `scripts/`:
  `analyze_paper_edge.py`, `backtest_fair_value.py` (with `--walk-forward` /
  `--sweep`), `capture_snapshots.py`, `run-continuous-capture.sh`,
  `capture_progress.py`.

### Changed
- Dependencies: bumped `arrow`/`parquet` 56→59, `tower-http` 0.6→0.7,
  `tokio-tungstenite` 0.28→0.30, and `@types/node` 22→26 (no source changes;
  all gates green). Deferred `typescript` 7 and `eslint` 10 — the current
  `eslint-config-next` tooling does not support them yet.
- Operator terminal migrated off Cloudflare/vinext/Wrangler to standard
  Next.js on Node; removed the unused D1 database, worker, and hosting scaffold.
- `cargo-deny` policy: `LicenseRef-Proprietary` + `publish = false` on all
  crates, `allow-wildcard-paths`, allow `CC0-1.0` / `CDLA-Permissive-2.0`,
  ignore `RUSTSEC-2024-0436` (transitive `paste`, no fix).

### Fixed
- CI: corrected the pinned `tla2tools.jar` SHA-256 (all TLA+ jobs had failed at
  download verification).
- Terminal: strict-mode type errors surfaced by `next build` (BigInt target,
  `Partial<Book>` narrowing, nullable `short()` argument).
- Terminal: the window paper analytics matched decision actions in PascalCase
  while the journal serializes them snake_case, so entry, exit and no-trade
  counts were always zero and the tape filters matched nothing.
- Window engine: a position that exited into a reverse candidate never booked
  its realized result, and the replacement position overwrote it.

### Notes
- Everything remains offline, read-only, and paper-only. No credential, signing,
  or order-submission capability was added.
