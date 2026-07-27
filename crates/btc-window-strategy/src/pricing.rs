//! Edge and effective cost. Adaptive stake sizing lives in [`crate::sizing`].

use crate::config::{WindowConfig, CHEAP_TOKEN_REJECT_MICROS, HARD_REJECT_ASK_MICROS};
use crate::types::{MarketSecondSnapshot, OutcomeSide, RiskState};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_field_names)] // Keep explicit micros units at the financial boundary.
pub struct PricingView {
    pub ask_micros: i64,
    pub fee_micros: i64,
    pub slippage_micros: i64,
    pub effective_entry_cost_micros: i64,
    pub calibrated_probability_micros: i64,
    pub edge_micros: i64,
}

#[must_use]
pub fn effective_entry_cost(ask_micros: i64, fee_micros: i64, slippage_micros: i64) -> Option<i64> {
    ask_micros
        .checked_add(fee_micros)?
        .checked_add(slippage_micros)
}

#[must_use]
pub fn compute_pricing(
    ask_micros: i64,
    calibrated_probability_micros: i64,
    snapshot: &MarketSecondSnapshot,
) -> Option<PricingView> {
    let effective = effective_entry_cost(
        ask_micros,
        snapshot.fee_estimate_micros,
        snapshot.slippage_estimate_micros,
    )?;
    let edge = calibrated_probability_micros.checked_sub(effective)?;
    Some(PricingView {
        ask_micros,
        fee_micros: snapshot.fee_estimate_micros,
        slippage_micros: snapshot.slippage_estimate_micros,
        effective_entry_cost_micros: effective,
        calibrated_probability_micros,
        edge_micros: edge,
    })
}

#[must_use]
pub fn ask_policy_rejects(ask_micros: i64, has_complete_lock: bool) -> Option<&'static str> {
    if ask_micros <= CHEAP_TOKEN_REJECT_MICROS {
        return Some("CHEAP_TOKEN_REJECT");
    }
    if ask_micros >= HARD_REJECT_ASK_MICROS && !has_complete_lock {
        return Some("HIGH_ASK_ASYMMETRY_REJECT");
    }
    if ask_micros >= 950_000 && !has_complete_lock {
        return Some("EXTREME_ASK_REJECT");
    }
    None
}

#[must_use]
pub fn price_band_ok(cfg: &WindowConfig, ask_micros: i64) -> bool {
    ask_micros >= cfg.preferred_ask_lo_micros && ask_micros <= cfg.max_ask_micros
}

#[must_use]
pub fn edge_passes(cfg: &WindowConfig, edge_micros: i64) -> bool {
    edge_micros >= cfg.min_edge_micros
}

/// Calibrated probability for v1.
///
/// Market mid alone cannot clear ask+fees, so confirmed persistence, momentum,
/// BTC agreement and acceleration add fixed-point boosts. A later rebuild may
/// replace this function without changing ask-entry / bid-exit valuation.
#[must_use]
pub fn calibrated_probability(
    confidence_mid_micros: i64,
    persistence_secs: i64,
    mom_med_micros: i64,
    mom_long_micros: i64,
    reference_agrees: bool,
    confidence_delta_60s_micros: i64,
) -> i64 {
    let mut p = confidence_mid_micros;
    if persistence_secs >= 20 {
        p = p.saturating_add(30_000);
    }
    if mom_med_micros > 0 && mom_long_micros >= 0 {
        p = p.saturating_add(20_000);
    }
    if reference_agrees {
        p = p.saturating_add(20_000);
    }
    if confidence_delta_60s_micros > 0 {
        p = p.saturating_add(10_000);
    }
    p.clamp(0, 1_000_000)
}

#[must_use]
pub fn portfolio_blocks_entry(
    cfg: &WindowConfig,
    risk: &RiskState,
    now_ns: i64,
) -> Option<&'static str> {
    if risk.day_entries_blocked {
        return Some("DAILY_STOP_LIMIT");
    }
    if risk.window_paused_until_ns > now_ns {
        return Some("WINDOW_PAUSE_AFTER_STOPS");
    }
    let daily_limit = risk
        .bankroll_micros
        .saturating_mul(cfg.daily_loss_limit_bps)
        / 10_000;
    if risk.daily_loss_micros >= daily_limit && daily_limit > 0 {
        return Some("DAILY_LOSS_LIMIT");
    }
    None
}

#[must_use]
pub fn side_reference_agrees(side: OutcomeSide, reference_return_bps: i64) -> bool {
    match side {
        OutcomeSide::Up => reference_return_bps >= 0,
        OutcomeSide::Down => reference_return_bps <= 0,
    }
}
