//! Every-second evaluation engine. Proposals only — no capital, signing, or submit.

use crate::accounting::{
    realized_pnl_micros, settled_side, settlement_payout_micros, stake_micros, value_position,
};
use crate::config::WindowConfig;
use crate::features::{build_features, side_bid, FeatureHistory, SecondFeatures};
use crate::hedge::scan_complete_set;
use crate::journal::{from_candidate, no_trade, DecisionRecord, SettlementOutcome};
use crate::ledger::ClosedTrade;
use crate::policy::ActivePolicy;
use crate::position::{apply_position_action, manage_open_position};
use crate::pricing::portfolio_blocks_entry;
use crate::setups::evaluate_window_candidates;
use crate::sizing::{plan_size, SetupPerformance};
use crate::types::{
    DecisionAction, MarketSecondSnapshot, OpenPosition, OutcomeSide, PerformanceBook,
    PositionState, RiskState, WindowKind, STRATEGY_VERSION,
};
use std::collections::BTreeMap;

#[derive(Clone, Debug)]
pub struct EngineState {
    pub position: Option<OpenPosition>,
    pub risk: RiskState,
    pub history: FeatureHistory,
    pub checkpoint_favourite_at_120: Option<OutcomeSide>,
    pub opposite_flip_secs: i64,
    pub reverse_candidate_secs: i64,
    pub decisions: Vec<DecisionRecord>,
    /// Day setup performance used for adaptive stake (Spec 093).
    pub setup_stats: BTreeMap<String, SetupPerformance>,
    /// Active window policy (Spec 096); defaults when no file is active.
    pub policy: ActivePolicy,
    pending_close: Option<ClosedTrade>,
}

impl Default for EngineState {
    fn default() -> Self {
        Self {
            position: None,
            risk: RiskState::default(),
            history: FeatureHistory::default(),
            checkpoint_favourite_at_120: None,
            opposite_flip_secs: 0,
            reverse_candidate_secs: 0,
            decisions: Vec::new(),
            setup_stats: BTreeMap::new(),
            policy: ActivePolicy::compiled_defaults(),
            pending_close: None,
        }
    }
}

/// The journal record and ledger entry produced by settling one window.
#[derive(Clone, Debug)]
pub struct SettledWindow {
    pub record: DecisionRecord,
    pub closed: ClosedTrade,
}

#[derive(Clone, Debug)]
pub struct TickResult {
    pub decision: DecisionRecord,
    pub position: Option<OpenPosition>,
    /// Set on the tick a position closed, so the caller books it exactly once.
    pub closed: Option<ClosedTrade>,
}

impl EngineState {
    #[must_use]
    pub fn new(bankroll_micros: i64) -> Self {
        Self {
            risk: RiskState {
                bankroll_micros,
                ..RiskState::default()
            },
            ..Self::default()
        }
    }

    /// Restore a checkpointed position after a restart (Spec 092).
    ///
    /// Feature history is deliberately not restored; momentum, persistence and
    /// acceleration rebuild from live observation, so a resumed position is
    /// managed from cold features until history refills. ADR 0096 accepts that
    /// this can bias a resumed position toward an earlier protective exit.
    #[must_use]
    pub fn resume(risk: RiskState, position: OpenPosition) -> Self {
        Self {
            position: Some(position),
            risk,
            ..Self::default()
        }
    }

    /// Refresh the shared tradable bankroll and setup win rates used for sizing.
    pub fn sync_capital(
        &mut self,
        bankroll_micros: i64,
        setup_stats: BTreeMap<String, SetupPerformance>,
    ) {
        self.risk.bankroll_micros = bankroll_micros;
        self.setup_stats = setup_stats;
    }

    /// Refresh the active window policy (Spec 096).
    pub fn sync_policy(&mut self, policy: ActivePolicy) {
        self.policy = policy;
    }

    fn setup_perf(&self, setup_id: &str) -> Option<SetupPerformance> {
        self.setup_stats.get(setup_id).copied()
    }

    /// Process one market second. Open-position logic always runs before entries.
    pub fn on_every_second(&mut self, snapshot: &MarketSecondSnapshot) -> TickResult {
        let cfg = WindowConfig::for_window(snapshot.window);
        let window_label = snapshot.window.label();

        let Some(features) = build_features(snapshot, &mut self.history, self.position.as_ref())
        else {
            return self.push_result(no_trade_invalid(
                snapshot,
                window_label,
                "INVALID_BOOK",
                self.position.as_ref(),
            ));
        };

        if snapshot.elapsed_secs == 120 && snapshot.window == WindowKind::FifteenMinute {
            self.checkpoint_favourite_at_120 = Some(features.favourite);
        }
        self.track_flip(&features);

        if !features.quote_fresh || !features.top_depth_ok {
            let reason = if features.quote_fresh {
                "INSUFFICIENT_DEPTH"
            } else {
                "STALE_QUOTES"
            };
            return self.push_result(no_trade_invalid(
                snapshot,
                window_label,
                reason,
                self.position.as_ref(),
            ));
        }

        // Scanner may observe locks; it never forces a directional entry.
        let _complete = scan_complete_set(snapshot, 1_000_000);

        if self.position.is_some() {
            return self.manage_existing(snapshot, &features, &cfg, window_label);
        }
        self.evaluate_flat_entry(snapshot, &features, &cfg, window_label)
    }

    fn track_flip(&mut self, features: &SecondFeatures) {
        match &self.position {
            Some(pos) if features.favourite == pos.side => self.opposite_flip_secs = 0,
            Some(_) => self.opposite_flip_secs = self.opposite_flip_secs.saturating_add(1),
            None => self.opposite_flip_secs = 0,
        }
    }

    fn push_result(&mut self, decision: DecisionRecord) -> TickResult {
        let position = self.position.clone();
        let closed = self.pending_close.take();
        self.decisions.push(decision.clone());
        TickResult {
            decision,
            position,
            closed,
        }
    }

    fn manage_existing(
        &mut self,
        snapshot: &MarketSecondSnapshot,
        features: &SecondFeatures,
        cfg: &WindowConfig,
        window_label: &str,
    ) -> TickResult {
        let mut pos = self.position.clone().expect("checked");
        if pos.state == PositionState::ReverseCandidate {
            return self.manage_reverse_candidate(snapshot, features, cfg, window_label, &pos);
        }

        let action = manage_open_position(snapshot, features, &pos, cfg, self.opposite_flip_secs);
        let bid = side_bid(snapshot, pos.side);
        apply_position_action(&mut pos, &action, bid);

        let valuation = value_position(snapshot, &pos);
        let decision = DecisionRecord {
            strategy_version: STRATEGY_VERSION.into(),
            condition_id: snapshot.condition_id.clone(),
            asset: snapshot.asset,
            window: window_label.into(),
            elapsed_secs: snapshot.elapsed_secs,
            observed_at_ns: snapshot.observed_at_ns,
            action: action.action,
            reason: action.reason.clone(),
            rejection_reasons: Vec::new(),
            setup_id: Some(pos.setup_id.clone()),
            side: Some(format!("{:?}", pos.side).to_ascii_lowercase()),
            ask_micros: None,
            bid_micros: Some(bid),
            edge_micros: None,
            effective_cost_micros: Some(pos.effective_entry_cost_micros),
            quantity_micros: Some(pos.remaining_quantity_micros),
            stake_micros: Some(pos.stake_micros),
            mark_value_micros: Some(valuation.mark_value_micros),
            unrealized_pnl_micros: Some(valuation.unrealized_pnl_micros),
            realized_pnl_micros: Some(pos.realized_pnl_micros),
            hold_secs: Some(hold_secs(snapshot, &pos)),
            outcome: None,
            settlement_source: None,
            stake_tier: None,
            risk_bps: None,
            configuration_digest: pos.configuration_digest.clone(),
            position_state: pos.state,
            book: pos.book,
            paper_only: true,
            real_money: false,
        };

        if action.exit_all {
            self.apply_exit(snapshot, &mut pos, action.enter_reverse_candidate);
        } else if pos.state == PositionState::Locked || pos.remaining_quantity_micros > 0 {
            self.position = Some(pos);
        } else {
            self.position = None;
        }

        self.push_result(decision)
    }

    fn apply_exit(
        &mut self,
        snapshot: &MarketSecondSnapshot,
        pos: &mut OpenPosition,
        enter_reverse: bool,
    ) {
        // The shares are gone either way, so the result is booked here rather
        // than deferred. A reverse candidate is a fresh trade, not a
        // continuation of the one that just stopped out.
        self.book_realized(pos.realized_pnl_micros);
        self.pending_close = Some(ClosedTrade {
            setup_id: pos.setup_id.clone(),
            book: pos.book,
            asset: pos.asset,
            window: snapshot.window.label().into(),
            realized_pnl_micros: pos.realized_pnl_micros,
            outcome: None,
            observed_at_ns: snapshot.observed_at_ns,
            hold_secs: hold_secs(snapshot, pos),
        });

        if enter_reverse {
            pos.state = PositionState::ReverseCandidate;
            pos.remaining_quantity_micros = 0;
            pos.realized_pnl_micros = 0;
            self.position = Some(pos.clone());
            self.reverse_candidate_secs = 0;
            return;
        }
        self.position = None;
        self.risk.consecutive_full_stops = self.risk.consecutive_full_stops.saturating_add(1);
        self.risk.daily_stop_count = self.risk.daily_stop_count.saturating_add(1);
        if self.risk.consecutive_full_stops >= 2 {
            self.risk.window_paused_until_ns = snapshot
                .observed_at_ns
                .saturating_add(30 * 60 * 1_000_000_000);
        }
        if self.risk.daily_stop_count >= 3 {
            self.risk.day_entries_blocked = true;
        }
    }

    /// Fold a closed position's realized result into the risk state.
    ///
    /// `daily_loss_micros` counts losses as a positive magnitude because the
    /// portfolio limits compare it against a positive loss budget.
    fn book_realized(&mut self, realized_pnl_micros: i64) {
        self.risk.realized_pnl_micros = self
            .risk
            .realized_pnl_micros
            .saturating_add(realized_pnl_micros);
        if realized_pnl_micros < 0 {
            self.risk.daily_loss_micros = self
                .risk
                .daily_loss_micros
                .saturating_add(realized_pnl_micros.saturating_abs());
        }
    }

    /// Settle the window and close any position at its binary payout.
    ///
    /// Called when the market leaves the feed or its duration elapses. The
    /// outcome comes from the observed reference level (ADR 0095), so the
    /// record is explicitly sourced and carries no resolver authority.
    pub fn settle(&mut self, snapshot: &MarketSecondSnapshot) -> Option<SettledWindow> {
        // A reverse candidate holds no shares; its result was booked on exit.
        // Check before taking, so declining to settle never discards a position
        // that still has quantity to account for.
        if self.position.as_ref()?.remaining_quantity_micros <= 0 {
            self.position = None;
            return None;
        }
        let mut pos = self.position.take()?;
        let winning_side =
            settled_side(snapshot.window_open_micros, snapshot.reference_spot_micros);
        let won = pos.side == winning_side;
        let payout = settlement_payout_micros(won);
        let settled = realized_pnl_micros(
            pos.remaining_quantity_micros,
            payout,
            pos.effective_entry_cost_micros,
        );
        pos.realized_pnl_micros = pos.realized_pnl_micros.saturating_add(settled);
        pos.remaining_quantity_micros = 0;
        pos.state = PositionState::Flat;
        self.book_realized(pos.realized_pnl_micros);

        let record = DecisionRecord {
            strategy_version: STRATEGY_VERSION.into(),
            condition_id: snapshot.condition_id.clone(),
            asset: snapshot.asset,
            window: snapshot.window.label().into(),
            elapsed_secs: snapshot.elapsed_secs,
            observed_at_ns: snapshot.observed_at_ns,
            action: DecisionAction::Settle,
            reason: if won {
                "SETTLED_IN_THE_MONEY"
            } else {
                "SETTLED_OUT_OF_THE_MONEY"
            }
            .into(),
            rejection_reasons: Vec::new(),
            setup_id: Some(pos.setup_id.clone()),
            side: Some(format!("{:?}", pos.side).to_ascii_lowercase()),
            ask_micros: Some(pos.entry_ask_micros),
            bid_micros: Some(payout),
            edge_micros: None,
            effective_cost_micros: Some(pos.effective_entry_cost_micros),
            quantity_micros: Some(0),
            stake_micros: Some(pos.stake_micros),
            mark_value_micros: Some(0),
            unrealized_pnl_micros: Some(0),
            realized_pnl_micros: Some(pos.realized_pnl_micros),
            hold_secs: Some(hold_secs(snapshot, &pos)),
            outcome: Some(if won {
                SettlementOutcome::Win
            } else {
                SettlementOutcome::Loss
            }),
            settlement_source: Some("observed_reference".into()),
            stake_tier: None,
            risk_bps: None,
            configuration_digest: pos.configuration_digest.clone(),
            position_state: PositionState::Flat,
            book: pos.book,
            paper_only: true,
            real_money: false,
        };
        self.decisions.push(record.clone());
        Some(SettledWindow {
            closed: ClosedTrade {
                setup_id: pos.setup_id.clone(),
                book: pos.book,
                asset: pos.asset,
                window: snapshot.window.label().into(),
                realized_pnl_micros: pos.realized_pnl_micros,
                outcome: record.outcome,
                observed_at_ns: snapshot.observed_at_ns,
                hold_secs: hold_secs(snapshot, &pos),
            },
            record,
        })
    }

    fn manage_reverse_candidate(
        &mut self,
        snapshot: &MarketSecondSnapshot,
        features: &SecondFeatures,
        cfg: &WindowConfig,
        window_label: &str,
        pos: &OpenPosition,
    ) -> TickResult {
        self.reverse_candidate_secs = self.reverse_candidate_secs.saturating_add(1);
        if self.reverse_candidate_secs < cfg.reverse_confirm_secs {
            return self.push_result(no_trade(
                snapshot,
                window_label,
                "REVERSE_CONFIRMING",
                &[],
                PositionState::ReverseCandidate,
                pos.book,
            ));
        }

        let (candidate, rejects) = evaluate_window_candidates(
            snapshot,
            features,
            cfg,
            self.checkpoint_favourite_at_120,
            &self.policy,
        );
        if let Some(candidate) = candidate.filter(|c| c.side == features.favourite) {
            if !pos.reversal_used {
                if let Some(size) = plan_size(
                    cfg,
                    &self.risk,
                    candidate.pricing.ask_micros,
                    true,
                    self.setup_perf(&candidate.setup_id),
                ) {
                    let open = open_from_candidate(
                        snapshot,
                        &candidate,
                        size.quantity_micros,
                        true,
                        &self.policy.digest_hex,
                    );
                    let decision = from_candidate(
                        snapshot,
                        window_label,
                        &candidate,
                        &size,
                        DecisionAction::OpenReversed,
                        "REVERSE_CONFIRMED",
                        Some(self.policy.digest_hex.clone()),
                    );
                    self.position = Some(open);
                    self.reverse_candidate_secs = 0;
                    return self.push_result(decision);
                }
            }
        }

        self.position = None;
        self.reverse_candidate_secs = 0;
        self.push_result(no_trade(
            snapshot,
            window_label,
            "REVERSE_NOT_QUALIFIED",
            &rejects,
            PositionState::Flat,
            PerformanceBook::ActiveCore,
        ))
    }

    fn evaluate_flat_entry(
        &mut self,
        snapshot: &MarketSecondSnapshot,
        features: &SecondFeatures,
        cfg: &WindowConfig,
        window_label: &str,
    ) -> TickResult {
        if let Some(reason) = portfolio_blocks_entry(cfg, &self.risk, snapshot.observed_at_ns) {
            return self.push_result(no_trade(
                snapshot,
                window_label,
                reason,
                &[],
                PositionState::Flat,
                PerformanceBook::ActiveCore,
            ));
        }

        if in_discovery(snapshot) {
            return self.push_result(no_trade(
                snapshot,
                window_label,
                "DISCOVERY_NO_ENTRY",
                &[],
                PositionState::Flat,
                PerformanceBook::ActiveCore,
            ));
        }

        if snapshot.window == WindowKind::FifteenMinute && snapshot.elapsed_secs >= 840 {
            return self.push_result(no_trade(
                snapshot,
                window_label,
                "FINAL_MINUTE_NO_NEW_ENTRY",
                &[],
                PositionState::Flat,
                PerformanceBook::ActiveCore,
            ));
        }

        let (candidate, rejects) = evaluate_window_candidates(
            snapshot,
            features,
            cfg,
            self.checkpoint_favourite_at_120,
            &self.policy,
        );
        let Some(candidate) = candidate else {
            return self.push_result(no_trade(
                snapshot,
                window_label,
                "NO_SETUP_PASSED",
                &rejects,
                PositionState::Flat,
                PerformanceBook::ActiveCore,
            ));
        };

        let Some(size) = plan_size(
            cfg,
            &self.risk,
            candidate.pricing.ask_micros,
            candidate.experimental,
            self.setup_perf(&candidate.setup_id),
        ) else {
            return self.push_result(no_trade(
                snapshot,
                window_label,
                "SIZE_ZERO",
                &rejects,
                PositionState::Flat,
                candidate.book,
            ));
        };

        let open = open_from_candidate(
            snapshot,
            &candidate,
            size.quantity_micros,
            false,
            &self.policy.digest_hex,
        );
        let decision = from_candidate(
            snapshot,
            window_label,
            &candidate,
            &size,
            DecisionAction::Open,
            "ENTRY_FILLED_AT_ASK",
            Some(self.policy.digest_hex.clone()),
        );
        self.position = Some(open);
        self.risk.consecutive_full_stops = 0;
        self.push_result(decision)
    }
}

fn in_discovery(snapshot: &MarketSecondSnapshot) -> bool {
    match snapshot.window {
        WindowKind::FiveMinute => snapshot.elapsed_secs < 60,
        WindowKind::FifteenMinute => snapshot.elapsed_secs < 120,
        WindowKind::OneHour => snapshot.elapsed_secs < 900,
    }
}

fn no_trade_invalid(
    snapshot: &MarketSecondSnapshot,
    window_label: &str,
    reason: &str,
    position: Option<&OpenPosition>,
) -> DecisionRecord {
    no_trade(
        snapshot,
        window_label,
        reason,
        &[],
        position.map_or(PositionState::Flat, |p| p.state),
        PerformanceBook::InvalidIncomplete,
    )
}

/// Seconds a position has been held, from its own entry timestamp.
fn hold_secs(snapshot: &MarketSecondSnapshot, position: &OpenPosition) -> i64 {
    snapshot
        .observed_at_ns
        .saturating_sub(position.entry_observed_at_ns)
        / 1_000_000_000
}

fn open_from_candidate(
    snapshot: &MarketSecondSnapshot,
    candidate: &crate::setups::EntryCandidate,
    quantity_micros: i64,
    reversal: bool,
    configuration_digest: &str,
) -> OpenPosition {
    let effective_cost = candidate.pricing.effective_entry_cost_micros;
    OpenPosition {
        asset: snapshot.asset,
        side: candidate.side,
        quantity_micros,
        entry_ask_micros: candidate.pricing.ask_micros,
        effective_entry_cost_micros: effective_cost,
        entry_elapsed_secs: snapshot.elapsed_secs,
        entry_observed_at_ns: snapshot.observed_at_ns,
        stake_micros: stake_micros(quantity_micros, effective_cost),
        realized_pnl_micros: 0,
        peak_bid_micros: side_bid(snapshot, candidate.side),
        remaining_quantity_micros: quantity_micros,
        protective_bid_micros: None,
        partials_taken: 0,
        reversal_used: reversal,
        setup_id: if reversal {
            format!("reverse:{}", candidate.setup_id)
        } else {
            candidate.setup_id.clone()
        },
        book: if reversal {
            PerformanceBook::ShadowExperiment
        } else {
            candidate.book
        },
        state: PositionState::Open,
        hedge_opposite_filled: false,
        hedge_opposite_cost_micros: 0,
        configuration_digest: Some(configuration_digest.to_owned()),
    }
}
