//! Paper book aggregation by strategy, asset, and window (Spec 091).
//!
//! `ACTIVE_CORE` and `SHADOW_EXPERIMENT` are never summed together. A shadow
//! setup that looks profitable must not inflate the core book that a promotion
//! decision would read.

use crate::accounting::PositionValuation;
use crate::journal::SettlementOutcome;
use crate::types::{MarketAsset, OpenPosition, PerformanceBook};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Running totals for one performance book.
///
/// Distribution fields (`gross_win_micros`, `best_trade_micros`, drawdown,
/// streaks) exist because a win rate alone cannot tell an operator whether a
/// setup is worth capital: sixty percent winners that lose more per loss than
/// they make per win is a losing book.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct BookTotals {
    pub closed_trades: u64,
    pub wins: u64,
    pub losses: u64,
    pub realized_pnl_micros: i64,
    pub open_positions: u64,
    pub open_stake_micros: i64,
    pub unrealized_pnl_micros: i64,
    /// Sum of winning results.
    pub gross_win_micros: i64,
    /// Positive magnitude of losing results.
    pub gross_loss_micros: i64,
    pub best_trade_micros: i64,
    pub worst_trade_micros: i64,
    /// Summed holding time of closed trades, for an average hold.
    pub total_hold_secs: i64,
    /// Largest peak-to-trough fall of the realized equity curve, as a positive
    /// magnitude.
    pub max_drawdown_micros: i64,
    /// Highest realized equity reached, used to measure drawdown.
    pub peak_equity_micros: i64,
    /// Consecutive results: positive counts wins, negative counts losses.
    pub current_streak: i64,
    pub best_win_streak: i64,
    pub worst_loss_streak: i64,
}

impl BookTotals {
    /// Win rate in basis points, or `None` before any trade has closed.
    #[must_use]
    pub fn win_rate_bps(&self) -> Option<i64> {
        if self.closed_trades == 0 {
            return None;
        }
        let wins = i64::try_from(self.wins).unwrap_or(i64::MAX);
        let closed = i64::try_from(self.closed_trades).unwrap_or(1).max(1);
        Some(wins.saturating_mul(10_000) / closed)
    }

    /// Realized plus currently marked P&L.
    #[must_use]
    pub const fn total_pnl_micros(&self) -> i64 {
        self.realized_pnl_micros
            .saturating_add(self.unrealized_pnl_micros)
    }

    /// Mean winning result, or `None` with no wins yet.
    #[must_use]
    pub fn avg_win_micros(&self) -> Option<i64> {
        let wins = i64::try_from(self.wins).unwrap_or(i64::MAX);
        (wins > 0).then(|| self.gross_win_micros / wins)
    }

    /// Mean losing result as a positive magnitude, or `None` with no losses yet.
    #[must_use]
    pub fn avg_loss_micros(&self) -> Option<i64> {
        let losses = i64::try_from(self.losses).unwrap_or(i64::MAX);
        (losses > 0).then(|| self.gross_loss_micros / losses)
    }

    /// Mean realized result per closed trade, or `None` before any trade closes.
    #[must_use]
    pub fn expectancy_micros(&self) -> Option<i64> {
        let closed = i64::try_from(self.closed_trades).unwrap_or(i64::MAX);
        (closed > 0).then(|| self.realized_pnl_micros / closed)
    }

    /// Gross win over gross loss in basis points.
    ///
    /// `None` when nothing has closed or nothing has lost yet: a book with no
    /// losses has an undefined ratio, and reporting it as a huge number would
    /// read as a real edge on a two-trade sample.
    #[must_use]
    pub fn profit_factor_bps(&self) -> Option<i64> {
        if self.closed_trades == 0 || self.gross_loss_micros <= 0 {
            return None;
        }
        Some(
            self.gross_win_micros
                .saturating_mul(10_000)
                .saturating_div(self.gross_loss_micros),
        )
    }

    /// Mean holding time of closed trades, or `None` before any trade closes.
    #[must_use]
    pub fn avg_hold_secs(&self) -> Option<i64> {
        let closed = i64::try_from(self.closed_trades).unwrap_or(i64::MAX);
        (closed > 0).then(|| self.total_hold_secs / closed)
    }

    fn record_close(
        &mut self,
        realized_pnl_micros: i64,
        outcome: Option<SettlementOutcome>,
        hold_secs: i64,
    ) {
        self.closed_trades = self.closed_trades.saturating_add(1);
        self.realized_pnl_micros = self.realized_pnl_micros.saturating_add(realized_pnl_micros);
        self.total_hold_secs = self.total_hold_secs.saturating_add(hold_secs);
        // A settled window reports its own binary outcome. A discretionary exit
        // has no outcome, so the sign of the realized result is the only truth.
        let won = outcome.map_or(realized_pnl_micros > 0, |o| o == SettlementOutcome::Win);
        // Gross figures are magnitudes, so they stay meaningful as a ratio even
        // if a settlement is ever labelled a win while realizing a loss.
        if won {
            self.wins = self.wins.saturating_add(1);
            self.gross_win_micros = self
                .gross_win_micros
                .saturating_add(realized_pnl_micros.max(0));
            self.current_streak = self.current_streak.max(0).saturating_add(1);
            self.best_win_streak = self.best_win_streak.max(self.current_streak);
        } else {
            self.losses = self.losses.saturating_add(1);
            self.gross_loss_micros = self
                .gross_loss_micros
                .saturating_add(realized_pnl_micros.min(0).saturating_abs());
            self.current_streak = self.current_streak.min(0).saturating_sub(1);
            self.worst_loss_streak = self.worst_loss_streak.min(self.current_streak);
        }

        if self.closed_trades == 1 {
            self.best_trade_micros = realized_pnl_micros;
            self.worst_trade_micros = realized_pnl_micros;
        } else {
            self.best_trade_micros = self.best_trade_micros.max(realized_pnl_micros);
            self.worst_trade_micros = self.worst_trade_micros.min(realized_pnl_micros);
        }

        self.peak_equity_micros = self.peak_equity_micros.max(self.realized_pnl_micros);
        let drawdown = self
            .peak_equity_micros
            .saturating_sub(self.realized_pnl_micros);
        self.max_drawdown_micros = self.max_drawdown_micros.max(drawdown);
    }

    fn record_open(&mut self, valuation: &PositionValuation) {
        self.open_positions = self.open_positions.saturating_add(1);
        self.open_stake_micros = self
            .open_stake_micros
            .saturating_add(valuation.stake_micros);
        self.unrealized_pnl_micros = self
            .unrealized_pnl_micros
            .saturating_add(valuation.unrealized_pnl_micros);
    }
}

/// Derived quality measures for a book, computed once so every consumer reads
/// the same numbers instead of re-deriving them from raw sums.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct BookQuality {
    pub win_rate_bps: Option<i64>,
    pub avg_win_micros: Option<i64>,
    pub avg_loss_micros: Option<i64>,
    pub expectancy_micros: Option<i64>,
    pub profit_factor_bps: Option<i64>,
    pub avg_hold_secs: Option<i64>,
}

impl BookQuality {
    #[must_use]
    pub fn of(totals: &BookTotals) -> Self {
        Self {
            win_rate_bps: totals.win_rate_bps(),
            avg_win_micros: totals.avg_win_micros(),
            avg_loss_micros: totals.avg_loss_micros(),
            expectancy_micros: totals.expectancy_micros(),
            profit_factor_bps: totals.profit_factor_bps(),
            avg_hold_secs: totals.avg_hold_secs(),
        }
    }
}

/// One point on the realized-equity curve, booked when a trade closes.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct EquityPoint {
    pub observed_at_ns: i64,
    pub active_core_micros: i64,
    pub shadow_micros: i64,
}

/// Attribution for a single `setup_id`.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct StrategyStats {
    pub setup_id: String,
    pub book: String,
    pub totals: BookTotals,
    pub quality: BookQuality,
    pub win_rate_bps: Option<i64>,
    pub total_pnl_micros: i64,
    pub last_observed_at_ns: i64,
}

/// One open position rendered for operators, with live valuation attached.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OpenPositionView {
    pub condition_id: String,
    pub asset: String,
    pub window: String,
    pub side: String,
    pub setup_id: String,
    pub book: String,
    pub position_state: String,
    pub quantity_micros: i64,
    pub entry_ask_micros: i64,
    pub effective_cost_micros: i64,
    pub bid_micros: i64,
    pub stake_micros: i64,
    pub mark_value_micros: i64,
    pub unrealized_pnl_micros: i64,
    pub realized_pnl_micros: i64,
    pub total_pnl_micros: i64,
    pub hold_secs: i64,
    pub entry_observed_at_ns: i64,
}

/// A position that finished, reported once so it is booked exactly once.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ClosedTrade {
    pub setup_id: String,
    pub book: PerformanceBook,
    pub asset: MarketAsset,
    pub window: String,
    pub realized_pnl_micros: i64,
    /// Present when a window settled; absent for a discretionary exit.
    pub outcome: Option<SettlementOutcome>,
    pub observed_at_ns: i64,
    /// Seconds held from entry to close, for average-hold reporting.
    pub hold_secs: i64,
}

/// One live position handed to the ledger for marking.
#[derive(Clone, Copy, Debug)]
pub struct OpenPositionEntry<'a> {
    pub condition_id: &'a str,
    pub window: &'a str,
    pub position: &'a OpenPosition,
    pub valuation: PositionValuation,
}

/// The full published book: totals, attribution, and live positions.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct LedgerSnapshot {
    pub active_core: BookTotals,
    pub shadow_experiment: BookTotals,
    pub active_core_quality: BookQuality,
    pub shadow_experiment_quality: BookQuality,
    pub by_strategy: Vec<StrategyStats>,
    pub by_asset: Vec<(String, BookTotals)>,
    pub by_window: Vec<(String, BookTotals)>,
    pub open_positions: Vec<OpenPositionView>,
    pub equity_curve: Vec<EquityPoint>,
}

/// Accumulates closed results across ticks and re-marks open positions each tick.
///
/// Closed results are cumulative for the worker's lifetime; open valuations are
/// rebuilt from scratch every tick so a stale mark can never survive a
/// disconnect.
#[derive(Clone, Debug, Default)]
pub struct PaperLedger {
    active_core: BookTotals,
    shadow_experiment: BookTotals,
    strategies: BTreeMap<String, StrategyStats>,
    assets: BTreeMap<String, BookTotals>,
    windows: BTreeMap<String, BookTotals>,
    equity_curve: Vec<EquityPoint>,
}

/// Bound on retained equity points, so a long session cannot grow the status
/// file without limit.
const EQUITY_CURVE_LIMIT: usize = 512;

impl PaperLedger {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Day realized across both books — used for shared capital / withdraw.
    #[must_use]
    pub fn total_realized_micros(&self) -> i64 {
        self.active_core
            .realized_pnl_micros
            .saturating_add(self.shadow_experiment.realized_pnl_micros)
    }

    /// Setup win/loss samples for adaptive stake sizing.
    #[must_use]
    pub fn setup_performance(&self) -> BTreeMap<String, crate::sizing::SetupPerformance> {
        self.strategies
            .iter()
            .map(|(id, stats)| {
                (
                    id.clone(),
                    crate::sizing::SetupPerformance {
                        closed_trades: stats.totals.closed_trades,
                        wins: stats.totals.wins,
                    },
                )
            })
            .collect()
    }

    /// Book a position that closed this tick, by exit or by settlement.
    pub fn record_close(&mut self, trade: &ClosedTrade) {
        let pnl = trade.realized_pnl_micros;
        let outcome = trade.outcome;
        let held = trade.hold_secs;
        self.book_totals_mut(trade.book)
            .record_close(pnl, outcome, held);
        self.assets
            .entry(trade.asset.label().to_owned())
            .or_default()
            .record_close(pnl, outcome, held);
        self.windows
            .entry(window_key(trade.asset, &trade.window))
            .or_default()
            .record_close(pnl, outcome, held);

        let entry = self
            .strategies
            .entry(trade.setup_id.clone())
            .or_insert_with(|| StrategyStats {
                setup_id: trade.setup_id.clone(),
                book: book_label(trade.book).to_owned(),
                ..StrategyStats::default()
            });
        entry.totals.record_close(pnl, outcome, held);
        entry.last_observed_at_ns = entry.last_observed_at_ns.max(trade.observed_at_ns);
        // A bounded realized-equity curve, so the terminal can draw the shape of
        // the day rather than only its endpoint.
        self.equity_curve.push(EquityPoint {
            observed_at_ns: trade.observed_at_ns,
            active_core_micros: self.active_core.realized_pnl_micros,
            shadow_micros: self.shadow_experiment.realized_pnl_micros,
        });
        if self.equity_curve.len() > EQUITY_CURVE_LIMIT {
            // Drop the oldest half at once rather than shifting on every close.
            self.equity_curve.drain(..EQUITY_CURVE_LIMIT / 2);
        }
    }

    /// Rebuild the open side of the book and publish the full snapshot.
    #[must_use]
    pub fn snapshot(&self, open: &[OpenPositionEntry<'_>], observed_at_ns: i64) -> LedgerSnapshot {
        let mut active_core = self.active_core;
        let mut shadow = self.shadow_experiment;
        let mut assets = self.assets.clone();
        let mut windows = self.windows.clone();
        let mut strategies = self.strategies.clone();
        let mut positions = Vec::with_capacity(open.len());

        for entry in open {
            let position = entry.position;
            let valuation = &entry.valuation;
            let totals = if position.book == PerformanceBook::ActiveCore {
                &mut active_core
            } else {
                &mut shadow
            };
            totals.record_open(valuation);
            assets
                .entry(position.asset.label().to_owned())
                .or_default()
                .record_open(valuation);
            windows
                .entry(window_key(position.asset, entry.window))
                .or_default()
                .record_open(valuation);
            strategies
                .entry(position.setup_id.clone())
                .or_insert_with(|| StrategyStats {
                    setup_id: position.setup_id.clone(),
                    book: book_label(position.book).to_owned(),
                    ..StrategyStats::default()
                })
                .totals
                .record_open(valuation);

            positions.push(OpenPositionView {
                condition_id: entry.condition_id.to_owned(),
                asset: position.asset.label().to_owned(),
                window: entry.window.to_owned(),
                side: format!("{:?}", position.side).to_ascii_lowercase(),
                setup_id: position.setup_id.clone(),
                book: book_label(position.book).to_owned(),
                position_state: format!("{:?}", position.state),
                quantity_micros: position.remaining_quantity_micros,
                entry_ask_micros: position.entry_ask_micros,
                effective_cost_micros: position.effective_entry_cost_micros,
                bid_micros: valuation.bid_micros,
                stake_micros: valuation.stake_micros,
                mark_value_micros: valuation.mark_value_micros,
                unrealized_pnl_micros: valuation.unrealized_pnl_micros,
                realized_pnl_micros: valuation.realized_pnl_micros,
                total_pnl_micros: valuation.total_pnl_micros(),
                hold_secs: observed_at_ns.saturating_sub(position.entry_observed_at_ns)
                    / 1_000_000_000,
                entry_observed_at_ns: position.entry_observed_at_ns,
            });
        }

        let mut by_strategy: Vec<StrategyStats> = strategies
            .into_values()
            .map(|mut s| {
                s.win_rate_bps = s.totals.win_rate_bps();
                s.total_pnl_micros = s.totals.total_pnl_micros();
                s.quality = BookQuality::of(&s.totals);
                s
            })
            .collect();
        // Best realized-plus-marked result first; that is the ranking an
        // operator asking "which setup is working" actually wants.
        by_strategy.sort_by(|a, b| {
            b.total_pnl_micros
                .cmp(&a.total_pnl_micros)
                .then_with(|| a.setup_id.cmp(&b.setup_id))
        });

        LedgerSnapshot {
            active_core_quality: BookQuality::of(&active_core),
            shadow_experiment_quality: BookQuality::of(&shadow),
            active_core,
            shadow_experiment: shadow,
            by_strategy,
            by_asset: assets.into_iter().collect(),
            by_window: windows.into_iter().collect(),
            open_positions: positions,
            equity_curve: self.equity_curve.clone(),
        }
    }

    fn book_totals_mut(&mut self, book: PerformanceBook) -> &mut BookTotals {
        if book == PerformanceBook::ActiveCore {
            &mut self.active_core
        } else {
            &mut self.shadow_experiment
        }
    }
}

/// Window totals are keyed by asset so a BTC 5m result never lands in the
/// ETH 5m column.
fn window_key(asset: MarketAsset, window: &str) -> String {
    format!("{}-{window}", asset.label())
}

const fn book_label(book: PerformanceBook) -> &'static str {
    match book {
        PerformanceBook::ActiveCore => "ACTIVE_CORE",
        PerformanceBook::ShadowExperiment => "SHADOW_EXPERIMENT",
        PerformanceBook::RetiredLegacy => "RETIRED_LEGACY",
        PerformanceBook::InvalidIncomplete => "INVALID_INCOMPLETE",
    }
}
