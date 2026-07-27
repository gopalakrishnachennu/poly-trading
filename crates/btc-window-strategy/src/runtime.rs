//! Multi-market paper runtime: evaluate, journal, publish status.

use crate::accounting::value_position;
use crate::engine::EngineState;
use crate::feed::{market_to_snapshot, strategy_markets, FeedError, WindowsFeed};
use crate::journal::DecisionRecord;
use crate::ledger::{LedgerSnapshot, OpenPositionEntry, PaperLedger};
use crate::policy::{ActivePolicy, PolicySource, PolicyStore};
use crate::recovery::{
    load_checkpoint, replay_journal, write_checkpoint, CheckpointEntry, PositionCheckpoint,
    RecoveryError, RecoverySummary,
};
use crate::sizing::CapitalState;
use crate::types::{
    DecisionAction, MarketSecondSnapshot, PerformanceBook, MICROS_PER_UNIT, STRATEGY_VERSION,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs::{create_dir_all, OpenOptions};
use std::io::Write;
use std::path::PathBuf;

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub bankroll_micros: i64,
    pub max_quote_age_ms: i64,
    pub fee_estimate_micros: i64,
    pub slippage_estimate_micros: i64,
    pub journal_dir: PathBuf,
    pub status_path: PathBuf,
    /// Spec 096 window policy root (`active.json` + `versions/`).
    pub policy_dir: PathBuf,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            bankroll_micros: 1_000 * MICROS_PER_UNIT,
            max_quote_age_ms: 5_000,
            fee_estimate_micros: 1_000,
            slippage_estimate_micros: 1_000,
            journal_dir: PathBuf::from("var/btc-window-paper"),
            status_path: PathBuf::from("var/btc-window-paper/status.json"),
            policy_dir: PathBuf::from("var/strategy-control"),
        }
    }
}

impl RuntimeConfig {
    #[must_use]
    pub fn checkpoint_path(&self) -> PathBuf {
        self.journal_dir.join("positions.json")
    }

    #[must_use]
    pub fn journal_path_for(&self, observed_at_ns: i64) -> PathBuf {
        self.journal_dir
            .join(format!("decisions-{}.jsonl", chrono_day(observed_at_ns)))
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[allow(clippy::struct_excessive_bools)] // Explicit authority flags for operator status.
pub struct RuntimeStatus {
    pub paper_only: bool,
    pub real_money: bool,
    pub strategy_version: String,
    pub live_trading_authority: bool,
    pub paper_week_capital_authority: bool,
    pub observed_at_ns: i64,
    pub markets_seen: u64,
    pub decisions_this_tick: u64,
    pub open_positions: u64,
    pub assets_evaluated: Vec<String>,
    pub settlements_this_tick: u64,
    pub ledger: LedgerSnapshot,
    /// What boot recovery reconstructed from the journal and checkpoint.
    pub recovery: RecoverySummary,
    /// Positions settled from a persisted snapshot because their window ended
    /// while the worker was down.
    pub settled_on_recovery: u64,
    /// Durability failures. Non-empty means the worker must stop: the journal is
    /// no longer a complete record of what the engine decided.
    pub journal_errors: Vec<String>,
    /// Shared paper capital: principal, tradable bankroll, and 50% withdraws.
    pub capital: CapitalState,
    pub tradable_bankroll_micros: i64,
    pub net_realized_micros: i64,
    pub withdraw_target_micros: i64,
    pub withdraw_progress_bps: i64,
    /// Spec 096 active (or default) policy digest.
    #[serde(default)]
    pub configuration_digest: String,
    #[serde(default)]
    pub policy_source: String,
    pub last_actions: Vec<StatusAction>,
    pub reject_reasons: Vec<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct StatusAction {
    pub condition_id: String,
    pub asset: String,
    pub window: String,
    pub action: String,
    pub reason: String,
    pub book: String,
}

#[derive(Debug)]
pub struct PaperRuntime {
    pub config: RuntimeConfig,
    engines: BTreeMap<String, EngineState>,
    window_open: BTreeMap<String, i64>,
    /// Last snapshot per condition, retained so a window that leaves the feed
    /// can still be settled against the level we actually observed.
    last_snapshots: BTreeMap<String, MarketSecondSnapshot>,
    ledger: PaperLedger,
    capital: CapitalState,
    policy: ActivePolicy,
    pub status: RuntimeStatus,
}

impl PaperRuntime {
    #[must_use]
    pub fn new(config: RuntimeConfig) -> Self {
        let capital = CapitalState::new(config.bankroll_micros);
        let policy = ActivePolicy::compiled_defaults();
        Self {
            status: RuntimeStatus {
                paper_only: true,
                real_money: false,
                strategy_version: crate::types::STRATEGY_VERSION.into(),
                live_trading_authority: false,
                paper_week_capital_authority: false,
                capital: capital.clone(),
                tradable_bankroll_micros: capital.tradable_bankroll_micros(0),
                net_realized_micros: 0,
                withdraw_target_micros: capital.withdraw_target_micros(),
                withdraw_progress_bps: 0,
                configuration_digest: policy.digest_hex.clone(),
                policy_source: policy_source_label(policy.source).into(),
                ..RuntimeStatus::default()
            },
            config,
            engines: BTreeMap::new(),
            window_open: BTreeMap::new(),
            last_snapshots: BTreeMap::new(),
            ledger: PaperLedger::new(),
            capital,
            policy,
        }
    }

    /// Boot with the book rebuilt from disk (Spec 092).
    ///
    /// Replays the day's journal into the ledger so totals are day-scoped, then
    /// restores checkpointed open positions into their engines. Positions whose
    /// window has already ended are settled on the first [`Self::on_feed`] call,
    /// which sees them as departed conditions.
    ///
    /// # Errors
    ///
    /// Returns [`RecoveryError`] if the journal contains an unparseable line, or
    /// the checkpoint is corrupt or written by another engine version. Recovery
    /// is all-or-nothing: a partially recovered financial book is worse than
    /// refusing to start.
    pub fn recover(config: RuntimeConfig, observed_at_ns: i64) -> Result<Self, RecoveryError> {
        let policy = PolicyStore::new(&config.policy_dir)
            .load_active()
            .map_err(|e| RecoveryError::CorruptPolicy {
                path: config.policy_dir.display().to_string(),
                detail: e.to_string(),
            })?;
        let mut runtime = Self::new(config);
        runtime.install_policy(policy);
        let mut summary = RecoverySummary::default();

        let journal = runtime.config.journal_path_for(observed_at_ns);
        replay_journal(&journal, &mut runtime.ledger, &mut summary)?;

        let checkpoint_path = runtime.config.checkpoint_path();
        if let Some(checkpoint) = load_checkpoint(&checkpoint_path, STRATEGY_VERSION)? {
            summary.checkpoint_present = true;
            for entry in checkpoint.entries {
                summary.positions_restored = summary.positions_restored.saturating_add(1);
                runtime
                    .window_open
                    .insert(entry.condition_id.clone(), entry.window_open_micros);
                runtime
                    .last_snapshots
                    .insert(entry.condition_id.clone(), entry.last_snapshot);
                runtime.engines.insert(
                    entry.condition_id,
                    EngineState::resume(entry.risk, entry.position),
                );
            }
        }

        // Replay may already clear a 50% profit checkpoint — book it before the
        // first live tick so sizing sees the post-withdraw bankroll.
        let realized = runtime.ledger.total_realized_micros();
        let _ = runtime
            .capital
            .apply_withdraw_checkpoints(realized, observed_at_ns);
        runtime.refresh_capital_views(realized);
        runtime.status.recovery = summary;
        Ok(runtime)
    }

    /// Evaluate every configured window market in one feed tick.
    pub fn on_feed(&mut self, feed: &WindowsFeed, observed_at_ns: i64) -> Vec<DecisionRecord> {
        let mut decisions = Vec::new();
        let mut last_actions = Vec::new();
        let mut rejects = Vec::new();
        let mut assets = std::collections::BTreeSet::new();
        let mut journal_errors = Vec::new();
        let recovery = self.status.recovery.clone();
        // A restored position whose window already ended is settled here, on the
        // first tick, because it is absent from the live feed.
        let recovering = self.status.observed_at_ns == 0 && recovery.positions_restored > 0;
        let markets = strategy_markets(feed);
        let markets_seen = markets.len() as u64;

        let active: std::collections::BTreeSet<String> =
            markets.iter().map(|m| m.cond.clone()).collect();
        let settlements = self.settle_departed(&active, &mut decisions, &mut journal_errors);
        let settled_on_recovery = if recovering {
            settlements
        } else {
            self.status.settled_on_recovery
        };

        // Shared capital + setup win rates drive every engine's stake this tick.
        self.sync_engines_capital();
        self.sync_engines_policy();

        for market in markets {
            let mut snapshot = match market_to_snapshot(
                market,
                observed_at_ns,
                self.config.max_quote_age_ms,
                self.config.fee_estimate_micros,
                self.config.slippage_estimate_micros,
                None,
            ) {
                Ok(s) => s,
                Err(FeedError::UnsupportedAsset) => continue,
                Err(err) => {
                    rejects.push(format!("{}:{err}", market.cond));
                    continue;
                }
            };

            let open = *self
                .window_open
                .entry(market.cond.clone())
                .or_insert(snapshot.reference_spot_micros);
            if open > 0 {
                snapshot.window_open_micros = open;
            }
            assets.insert(snapshot.asset.label().to_owned());

            let bankroll = self.status.tradable_bankroll_micros;
            let setup_stats = self.ledger.setup_performance();
            let engine = self
                .engines
                .entry(market.cond.clone())
                .or_insert_with(|| EngineState::new(bankroll));
            // New engines created mid-tick still need today's setup stats.
            engine.sync_capital(bankroll, setup_stats);
            engine.sync_policy(self.policy.clone());
            let tick = engine.on_every_second(&snapshot);
            if let Some(closed) = tick.closed {
                self.ledger.record_close(&closed);
            }
            last_actions.push(StatusAction {
                condition_id: market.cond.clone(),
                asset: snapshot.asset.label().to_owned(),
                window: market.window.clone(),
                action: format!("{:?}", tick.decision.action),
                reason: tick.decision.reason.clone(),
                book: format!("{:?}", tick.decision.book),
            });
            if let Err(err) = self.append_decision(&tick.decision) {
                journal_errors.push(format!("append:{}:{err}", market.cond));
            }
            decisions.push(tick.decision);
            self.last_snapshots.insert(market.cond.clone(), snapshot);
        }

        let realized = self.ledger.total_realized_micros();
        let _ = self
            .capital
            .apply_withdraw_checkpoints(realized, observed_at_ns);
        self.refresh_capital_views(realized);

        // Checkpoint before publishing status: an open position that is not on
        // disk is one a restart would abandon.
        if let Err(err) = self.write_position_checkpoint(observed_at_ns) {
            journal_errors.push(format!("checkpoint:{err}"));
        }

        let ledger = self.mark_open_book(observed_at_ns);
        self.publish_status(
            observed_at_ns,
            markets_seen,
            decisions.len() as u64,
            settlements,
            assets.into_iter().collect(),
            ledger,
            recovery,
            settled_on_recovery,
            journal_errors,
            last_actions,
            rejects,
            realized,
        );
        let _ = self.write_status();
        decisions
    }

    #[allow(clippy::too_many_arguments)]
    fn publish_status(
        &mut self,
        observed_at_ns: i64,
        markets_seen: u64,
        decisions_this_tick: u64,
        settlements_this_tick: u64,
        assets_evaluated: Vec<String>,
        ledger: LedgerSnapshot,
        recovery: RecoverySummary,
        settled_on_recovery: u64,
        journal_errors: Vec<String>,
        last_actions: Vec<StatusAction>,
        reject_reasons: Vec<String>,
        realized: i64,
    ) {
        self.status = RuntimeStatus {
            paper_only: true,
            real_money: false,
            strategy_version: crate::types::STRATEGY_VERSION.into(),
            live_trading_authority: false,
            paper_week_capital_authority: false,
            observed_at_ns,
            markets_seen,
            decisions_this_tick,
            open_positions: ledger.open_positions.len() as u64,
            assets_evaluated,
            settlements_this_tick,
            ledger,
            recovery,
            settled_on_recovery,
            journal_errors,
            capital: self.capital.clone(),
            tradable_bankroll_micros: self.capital.tradable_bankroll_micros(realized),
            net_realized_micros: self.capital.net_realized_micros(realized),
            withdraw_target_micros: self.capital.withdraw_target_micros(),
            withdraw_progress_bps: self.capital.progress_bps(realized),
            configuration_digest: self.policy.digest_hex.clone(),
            policy_source: policy_source_label(self.policy.source).into(),
            last_actions,
            reject_reasons,
        };
    }

    fn refresh_capital_views(&mut self, realized_pnl_micros: i64) {
        self.status.capital = self.capital.clone();
        self.status.tradable_bankroll_micros =
            self.capital.tradable_bankroll_micros(realized_pnl_micros);
        self.status.net_realized_micros = self.capital.net_realized_micros(realized_pnl_micros);
        self.status.withdraw_target_micros = self.capital.withdraw_target_micros();
        self.status.withdraw_progress_bps = self.capital.progress_bps(realized_pnl_micros);
    }

    fn install_policy(&mut self, policy: ActivePolicy) {
        self.status
            .configuration_digest
            .clone_from(&policy.digest_hex);
        self.status.policy_source = policy_source_label(policy.source).into();
        self.policy = policy;
    }

    fn sync_engines_policy(&mut self) {
        // Soft-reload each tick so an activate takes effect on the next signal.
        // Corrupt reload keeps the last good policy and does not halt the book
        // mid-session (boot already fail-closed via recover).
        if let Ok(policy) = PolicyStore::new(&self.config.policy_dir).load_active() {
            self.install_policy(policy);
        }
        for engine in self.engines.values_mut() {
            engine.sync_policy(self.policy.clone());
        }
    }

    fn sync_engines_capital(&mut self) {
        let realized = self.ledger.total_realized_micros();
        let bankroll = self.capital.tradable_bankroll_micros(realized);
        let setup_stats = self.ledger.setup_performance();
        self.status.tradable_bankroll_micros = bankroll;
        for engine in self.engines.values_mut() {
            engine.sync_capital(bankroll, setup_stats.clone());
        }
    }

    /// True when a durability failure means the worker must stop.
    #[must_use]
    pub fn must_halt(&self) -> bool {
        !self.status.journal_errors.is_empty()
    }

    /// Seed a funded position that has no observed snapshot, so the unsettleable
    /// path can be exercised. Only reachable in tests.
    #[cfg(test)]
    pub(crate) fn force_position_without_snapshot(
        &mut self,
        condition_id: &str,
        position: crate::types::OpenPosition,
    ) {
        let mut engine = EngineState::new(self.config.bankroll_micros);
        engine.position = Some(position);
        self.engines.insert(condition_id.to_owned(), engine);
    }

    /// Persist every open position with the snapshot needed to settle it.
    fn write_position_checkpoint(&self, observed_at_ns: i64) -> Result<(), String> {
        let mut entries = Vec::new();
        for (cond, engine) in &self.engines {
            let Some(position) = engine.position.as_ref() else {
                continue;
            };
            let Some(snapshot) = self.last_snapshots.get(cond) else {
                continue;
            };
            entries.push(CheckpointEntry {
                condition_id: cond.clone(),
                position: position.clone(),
                risk: engine.risk.clone(),
                window_open_micros: self.window_open.get(cond).copied().unwrap_or_default(),
                last_snapshot: snapshot.clone(),
            });
        }
        write_checkpoint(
            &self.config.checkpoint_path(),
            &PositionCheckpoint {
                strategy_version: STRATEGY_VERSION.into(),
                written_at_ns: observed_at_ns,
                entries,
            },
        )
    }

    /// Settle and retire conditions that are no longer in the feed.
    ///
    /// A window disappears when it expires, so its last observed snapshot is
    /// the closing state. Dropping the engine without settling would silently
    /// discard an open position and its result.
    fn settle_departed(
        &mut self,
        active: &std::collections::BTreeSet<String>,
        decisions: &mut Vec<DecisionRecord>,
        journal_errors: &mut Vec<String>,
    ) -> u64 {
        let departed: Vec<String> = self
            .engines
            .keys()
            .filter(|cond| !active.contains(*cond))
            .cloned()
            .collect();

        let mut settled = 0_u64;
        for cond in departed {
            let snapshot = self.last_snapshots.get(&cond).cloned();
            // Retiring an engine that still holds shares without a snapshot to
            // settle against would abandon a funded position, so keep it until a
            // snapshot arrives and report the condition instead of dropping it.
            let holds_shares = self
                .engines
                .get(&cond)
                .and_then(|e| e.position.as_ref())
                .is_some_and(|p| p.remaining_quantity_micros > 0);
            if snapshot.is_none() && holds_shares {
                journal_errors.push(format!("unsettleable:{cond}:no observed snapshot"));
                continue;
            }

            if let (Some(mut engine), Some(snapshot)) = (self.engines.remove(&cond), snapshot) {
                if let Some(settlement) = engine.settle(&snapshot) {
                    self.ledger.record_close(&settlement.closed);
                    if let Err(err) = self.append_decision(&settlement.record) {
                        journal_errors.push(format!("settle:{cond}:{err}"));
                    }
                    decisions.push(settlement.record);
                    settled = settled.saturating_add(1);
                }
            }
            self.window_open.remove(&cond);
            self.last_snapshots.remove(&cond);
        }
        settled
    }

    /// Re-mark every open position against the book we just observed.
    fn mark_open_book(&self, observed_at_ns: i64) -> LedgerSnapshot {
        let mut entries = Vec::new();
        for (cond, engine) in &self.engines {
            let Some(position) = engine.position.as_ref() else {
                continue;
            };
            let Some(snapshot) = self.last_snapshots.get(cond) else {
                continue;
            };
            entries.push(OpenPositionEntry {
                condition_id: cond,
                window: snapshot.window.label(),
                position,
                valuation: value_position(snapshot, position),
            });
        }
        self.ledger.snapshot(&entries, observed_at_ns)
    }

    fn append_decision(&self, decision: &DecisionRecord) -> Result<(), String> {
        create_dir_all(&self.config.journal_dir).map_err(|e| e.to_string())?;
        let path = self.config.journal_path_for(decision.observed_at_ns);
        let mut file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|e| e.to_string())?;
        let line = serde_json::to_string(decision).map_err(|e| e.to_string())?;
        writeln!(file, "{line}").map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        Ok(())
    }

    fn write_status(&self) -> Result<(), String> {
        if let Some(parent) = self.config.status_path.parent() {
            create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let tmp = self.config.status_path.with_extension("json.tmp");
        let payload = serde_json::to_vec_pretty(&self.status).map_err(|e| e.to_string())?;
        std::fs::write(&tmp, payload).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &self.config.status_path).map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn chrono_day(observed_at_ns: i64) -> String {
    let secs = observed_at_ns / 1_000_000_000;
    let days = secs.div_euclid(86_400);
    let mut y = 1970_i64;
    let mut rem = days;
    loop {
        let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
        let year_days = if leap { 366 } else { 365 };
        if rem < year_days {
            break;
        }
        rem -= year_days;
        y += 1;
    }
    let leap = y % 4 == 0 && (y % 100 != 0 || y % 400 == 0);
    let mdays = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month = 1_i64;
    for (idx, len) in mdays.iter().enumerate() {
        let month_i = i64::try_from(idx).unwrap_or(0) + 1;
        if rem < *len {
            month = month_i;
            break;
        }
        rem -= *len;
        month = month_i + 1;
    }
    let day = rem + 1;
    format!("{y:04}-{month:02}-{day:02}")
}

/// Helper for tests / reporting: count `ACTIVE_CORE` opens in a decision batch.
#[must_use]
pub fn count_active_core_opens(decisions: &[DecisionRecord]) -> usize {
    decisions
        .iter()
        .filter(|d| d.action == DecisionAction::Open && d.book == PerformanceBook::ActiveCore)
        .count()
}

fn policy_source_label(source: PolicySource) -> &'static str {
    match source {
        PolicySource::CompiledDefaults => "compiled_defaults",
        PolicySource::ActiveFile => "active_file",
    }
}
