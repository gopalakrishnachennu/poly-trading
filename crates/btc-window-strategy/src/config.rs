//! Per-window configuration for paper trading.

use crate::types::{ConfidenceLabel, WindowKind, MICROS_PER_UNIT, POINT_MICROS};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WindowConfig {
    pub window: WindowKind,
    pub label: ConfidenceLabel,
    pub min_edge_micros: i64,
    pub preferred_ask_lo_micros: i64,
    pub preferred_ask_hi_micros: i64,
    pub max_ask_micros: i64,
    pub max_spread_micros: i64,
    pub persistence_secs: i64,
    pub reverse_confirm_secs: i64,
    pub emergency_stop_lo_micros: i64,
    pub emergency_stop_hi_micros: i64,
    pub profit_protect_micros: i64,
    pub profit_partial1_micros: i64,
    pub profit_partial2_micros: i64,
    pub trail_lo_micros: i64,
    pub trail_hi_micros: i64,
    pub normal_risk_bps_lo: i64,
    pub normal_risk_bps_hi: i64,
    pub high_ask_threshold_micros: i64,
    pub max_market_exposure_bps: i64,
    pub daily_loss_limit_bps: i64,
}

impl WindowConfig {
    #[must_use]
    pub const fn for_window(window: WindowKind) -> Self {
        match window {
            WindowKind::FiveMinute => Self {
                window,
                label: ConfidenceLabel::CoreCandidate,
                min_edge_micros: 4 * POINT_MICROS,
                preferred_ask_lo_micros: 580_000,
                preferred_ask_hi_micros: 780_000,
                max_ask_micros: 820_000,
                max_spread_micros: 2 * POINT_MICROS,
                persistence_secs: 20,
                reverse_confirm_secs: 5,
                // Widened from 6/8 -> 8/10 (matches the 15m band): 48h paper
                // evidence showed EMERGENCY_PRICE_STOP firing on ordinary 5m
                // noise (256 fires / -$848 over 48h, 0% win by definition,
                // the single largest cross-setup cost). See
                // docs/adr/0101-widen-short-window-emergency-stop-bands.md.
                emergency_stop_lo_micros: 8 * POINT_MICROS,
                emergency_stop_hi_micros: 10 * POINT_MICROS,
                profit_protect_micros: 5 * POINT_MICROS,
                profit_partial1_micros: 8 * POINT_MICROS,
                profit_partial2_micros: 12 * POINT_MICROS,
                trail_lo_micros: 3 * POINT_MICROS,
                trail_hi_micros: 4 * POINT_MICROS,
                // Legacy band kept for diagnostics; adaptive stake (Spec 093)
                // chooses risk from setup win rate instead of this midpoint.
                normal_risk_bps_lo: 150,
                normal_risk_bps_hi: 1_000,
                high_ask_threshold_micros: 780_000,
                max_market_exposure_bps: 1_200,
                daily_loss_limit_bps: 2_500,
            },
            WindowKind::FifteenMinute => Self {
                window,
                label: ConfidenceLabel::CoreCandidate,
                min_edge_micros: 5 * POINT_MICROS,
                preferred_ask_lo_micros: 580_000,
                preferred_ask_hi_micros: 800_000,
                max_ask_micros: 840_000,
                max_spread_micros: 2 * POINT_MICROS,
                persistence_secs: 60,
                reverse_confirm_secs: 20,
                // Widened from 8/10 -> 10/12 for the same reason as 5m above.
                emergency_stop_lo_micros: 10 * POINT_MICROS,
                emergency_stop_hi_micros: 12 * POINT_MICROS,
                profit_protect_micros: 7 * POINT_MICROS,
                profit_partial1_micros: 10 * POINT_MICROS,
                profit_partial2_micros: 15 * POINT_MICROS,
                trail_lo_micros: 4 * POINT_MICROS,
                trail_hi_micros: 5 * POINT_MICROS,
                normal_risk_bps_lo: 150,
                normal_risk_bps_hi: 1_000,
                high_ask_threshold_micros: 780_000,
                max_market_exposure_bps: 1_200,
                daily_loss_limit_bps: 2_500,
            },
            WindowKind::OneHour => Self {
                window,
                label: ConfidenceLabel::Shadow,
                min_edge_micros: 6 * POINT_MICROS,
                preferred_ask_lo_micros: 550_000,
                preferred_ask_hi_micros: 800_000,
                max_ask_micros: 880_000,
                max_spread_micros: 2 * POINT_MICROS,
                persistence_secs: 300,
                reverse_confirm_secs: 120,
                emergency_stop_lo_micros: 10 * POINT_MICROS,
                emergency_stop_hi_micros: 15 * POINT_MICROS,
                profit_protect_micros: 10 * POINT_MICROS,
                profit_partial1_micros: 15 * POINT_MICROS,
                profit_partial2_micros: 20 * POINT_MICROS,
                trail_lo_micros: 5 * POINT_MICROS,
                trail_hi_micros: 7 * POINT_MICROS,
                normal_risk_bps_lo: 150,
                normal_risk_bps_hi: 300,
                high_ask_threshold_micros: 780_000,
                max_market_exposure_bps: 600,
                daily_loss_limit_bps: 2_500,
            },
        }
    }
}

/// Sequential / complete-set lock threshold (0.97).
pub const SEQUENTIAL_LOCK_MAX_COST_MICROS: i64 = 970_000;

/// Minimum complete-set locked edge (0.02).
pub const COMPLETE_SET_MIN_LOCK_EDGE_MICROS: i64 = 20_000;

/// Hard reject asks at or above this unless a genuine lock exists.
pub const HARD_REJECT_ASK_MICROS: i64 = 900_000;

/// Cheap-token default reject (retired round-trip lesson).
pub const CHEAP_TOKEN_REJECT_MICROS: i64 = 200_000;

#[must_use]
pub fn payout_micros() -> i64 {
    MICROS_PER_UNIT
}
