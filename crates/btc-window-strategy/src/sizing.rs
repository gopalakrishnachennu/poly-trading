//! Adaptive paper stake from setup win rate (Spec 093 / ADR 0097).
//!
//! A human with a fixed principal does not bet the same on a cold setup and a
//! setup that has been winning all day. Tiers below are basis points of the
//! shared tradable bankroll.

use crate::config::WindowConfig;
use crate::types::{ConfidenceLabel, PerformanceBook, RiskState};
use serde::{Deserialize, Serialize};

/// Minimum closed trades before a setup leaves the unproven tier.
pub const MIN_SAMPLE_FOR_TIER: u64 = 5;

/// Stake tiers in basis points of tradable bankroll.
pub const STAKE_BPS_UNPROVEN: i64 = 200; // 2%
pub const STAKE_BPS_WEAK: i64 = 150; // 1.5%
pub const STAKE_BPS_AVERAGE: i64 = 300; // 3%
pub const STAKE_BPS_GOOD: i64 = 600; // 6%
pub const STAKE_BPS_STRONG: i64 = 1_000; // 10%
pub const STAKE_BPS_SHADOW_CAP: i64 = 300; // 3% hard cap for shadow/experimental

/// Observed performance for one setup, used only for sizing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct SetupPerformance {
    pub closed_trades: u64,
    pub wins: u64,
}

impl SetupPerformance {
    #[must_use]
    pub fn win_rate_bps(self) -> Option<i64> {
        if self.closed_trades == 0 {
            return None;
        }
        let wins = i64::try_from(self.wins).unwrap_or(i64::MAX);
        let closed = i64::try_from(self.closed_trades).unwrap_or(1).max(1);
        Some(wins.saturating_mul(10_000) / closed)
    }
}

/// Human-readable stake tier for journals and the terminal.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StakeTier {
    Unproven,
    Weak,
    Average,
    Good,
    Strong,
}

impl StakeTier {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Unproven => "unproven",
            Self::Weak => "weak",
            Self::Average => "average",
            Self::Good => "good",
            Self::Strong => "strong",
        }
    }
}

/// Choose stake tier from sample size and win rate.
#[must_use]
pub fn stake_tier(perf: Option<SetupPerformance>) -> StakeTier {
    let Some(perf) = perf else {
        return StakeTier::Unproven;
    };
    if perf.closed_trades < MIN_SAMPLE_FOR_TIER {
        return StakeTier::Unproven;
    }
    match perf.win_rate_bps() {
        Some(bps) if bps >= 6_500 => StakeTier::Strong,
        Some(bps) if bps >= 5_500 => StakeTier::Good,
        Some(bps) if bps >= 4_500 => StakeTier::Average,
        Some(_) => StakeTier::Weak,
        None => StakeTier::Unproven,
    }
}

/// Risk budget in bps for this setup / book.
#[must_use]
pub fn stake_risk_bps(tier: StakeTier, experimental: bool) -> i64 {
    let core = match tier {
        StakeTier::Unproven => STAKE_BPS_UNPROVEN,
        StakeTier::Weak => STAKE_BPS_WEAK,
        StakeTier::Average => STAKE_BPS_AVERAGE,
        StakeTier::Good => STAKE_BPS_GOOD,
        StakeTier::Strong => STAKE_BPS_STRONG,
    };
    if experimental {
        (core / 2).clamp(STAKE_BPS_WEAK, STAKE_BPS_SHADOW_CAP)
    } else {
        core
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SizePlan {
    pub quantity_micros: i64,
    pub risk_amount_micros: i64,
    pub risk_bps: i64,
    pub stake_tier: StakeTier,
    pub book: PerformanceBook,
}

/// Size an entry from tradable bankroll and the setup's own win rate.
#[must_use]
pub fn plan_size(
    cfg: &WindowConfig,
    risk: &RiskState,
    ask_micros: i64,
    experimental: bool,
    setup_perf: Option<SetupPerformance>,
) -> Option<SizePlan> {
    if risk.bankroll_micros <= 0 {
        return None;
    }
    let tier = stake_tier(setup_perf);
    let mut risk_bps = stake_risk_bps(tier, experimental);
    if ask_micros > cfg.high_ask_threshold_micros {
        risk_bps = (risk_bps * 3) / 4;
    }
    let risk_amount = risk.bankroll_micros.saturating_mul(risk_bps) / 10_000;
    if risk_amount <= 0 {
        return None;
    }
    let denom = ask_micros.max(1);
    let quantity = risk_amount.saturating_mul(1_000_000) / denom;
    if quantity <= 0 {
        return None;
    }
    let market_cap = risk
        .bankroll_micros
        .saturating_mul(cfg.max_market_exposure_bps)
        / 10_000;
    let book = if experimental || matches!(cfg.label, ConfidenceLabel::Shadow) {
        PerformanceBook::ShadowExperiment
    } else {
        PerformanceBook::ActiveCore
    };
    let notional = quantity.saturating_mul(ask_micros) / 1_000_000;
    if notional > market_cap {
        let capped_qty = market_cap.saturating_mul(1_000_000) / denom;
        if capped_qty <= 0 {
            return None;
        }
        return Some(SizePlan {
            quantity_micros: capped_qty,
            risk_amount_micros: market_cap,
            risk_bps: cfg.max_market_exposure_bps,
            stake_tier: tier,
            book,
        });
    }
    Some(SizePlan {
        quantity_micros: quantity,
        risk_amount_micros: risk_amount,
        risk_bps,
        stake_tier: tier,
        book,
    })
}

/// Shared paper capital: principal, withdrawals, and tradable bankroll.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct CapitalState {
    pub principal_micros: i64,
    /// Withdraw when net realized reaches this fraction of principal (bps).
    pub profit_withdraw_bps: i64,
    pub withdrawn_micros: i64,
    pub withdraw_count: u64,
    pub last_withdraw_micros: i64,
    pub last_withdraw_at_ns: i64,
}

impl Default for CapitalState {
    fn default() -> Self {
        Self::new(1_000 * crate::types::MICROS_PER_UNIT)
    }
}

impl CapitalState {
    #[must_use]
    pub fn new(principal_micros: i64) -> Self {
        Self {
            principal_micros,
            profit_withdraw_bps: 5_000, // 50%
            withdrawn_micros: 0,
            withdraw_count: 0,
            last_withdraw_micros: 0,
            last_withdraw_at_ns: 0,
        }
    }

    /// Profit still sitting in the book (realized minus already withdrawn).
    #[must_use]
    pub fn net_realized_micros(&self, realized_pnl_micros: i64) -> i64 {
        realized_pnl_micros.saturating_sub(self.withdrawn_micros)
    }

    /// Next withdraw fires when net realized reaches this amount.
    #[must_use]
    pub fn withdraw_target_micros(&self) -> i64 {
        self.principal_micros
            .saturating_mul(self.profit_withdraw_bps)
            / 10_000
    }

    /// Progress toward the next 50% checkpoint, in bps of principal.
    #[must_use]
    pub fn progress_bps(&self, realized_pnl_micros: i64) -> i64 {
        let target = self.withdraw_target_micros().max(1);
        self.net_realized_micros(realized_pnl_micros)
            .saturating_mul(10_000)
            / target
    }

    /// Tradable bankroll used for sizing.
    #[must_use]
    pub fn tradable_bankroll_micros(&self, realized_pnl_micros: i64) -> i64 {
        // Floor at 10% of principal so a deep drawdown cannot zero every size.
        let floor = self.principal_micros.saturating_mul(1_000) / 10_000;
        self.principal_micros
            .saturating_add(realized_pnl_micros)
            .saturating_sub(self.withdrawn_micros)
            .max(floor.max(1))
    }

    /// Book as many 50% checkpoints as net realized currently funds.
    ///
    /// Returns the micros withdrawn on this call (0 if none).
    pub fn apply_withdraw_checkpoints(
        &mut self,
        realized_pnl_micros: i64,
        observed_at_ns: i64,
    ) -> i64 {
        let target = self.withdraw_target_micros();
        if target <= 0 {
            return 0;
        }
        let mut withdrawn_now = 0_i64;
        while self.net_realized_micros(realized_pnl_micros) >= target {
            self.withdrawn_micros = self.withdrawn_micros.saturating_add(target);
            self.withdraw_count = self.withdraw_count.saturating_add(1);
            self.last_withdraw_micros = target;
            self.last_withdraw_at_ns = observed_at_ns;
            withdrawn_now = withdrawn_now.saturating_add(target);
            // Guard against a stuck loop if realized somehow does not move.
            if withdrawn_now / target > 100 {
                break;
            }
        }
        withdrawn_now
    }
}
