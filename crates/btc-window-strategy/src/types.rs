//! Shared fixed-point types for the multi-window paper engine.
//!
//! One collateral dollar / probability unit = `1_000_000` micros.
//! One price "point" (1¢ on a $1 binary) = `10_000` micros.

use serde::{Deserialize, Serialize};

/// Micros in one whole unit ($1.00 / probability 1.0).
pub const MICROS_PER_UNIT: i64 = 1_000_000;

/// One percentage / price point on a $1 binary contract.
pub const POINT_MICROS: i64 = 10_000;

/// Immutable strategy version for `ACTIVE_CORE` / `SHADOW` journals.
pub const STRATEGY_VERSION: &str = "btc_window_engine_v1";

/// A reference asset with tradable UP/DOWN windows (Spec 091).
#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "UPPERCASE")]
pub enum MarketAsset {
    Btc,
    Eth,
}

impl MarketAsset {
    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::Btc => "BTC",
            Self::Eth => "ETH",
        }
    }

    /// Parse a feed asset symbol. Unknown assets are not evaluated.
    #[must_use]
    pub fn from_symbol(symbol: &str) -> Option<Self> {
        match symbol {
            "BTC" => Some(Self::Btc),
            "ETH" => Some(Self::Eth),
            _ => None,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WindowKind {
    FiveMinute,
    FifteenMinute,
    OneHour,
}

impl WindowKind {
    #[must_use]
    pub const fn duration_secs(self) -> i64 {
        match self {
            Self::FiveMinute => 300,
            Self::FifteenMinute => 900,
            Self::OneHour => 3_600,
        }
    }

    #[must_use]
    pub const fn label(self) -> &'static str {
        match self {
            Self::FiveMinute => "5m",
            Self::FifteenMinute => "15m",
            Self::OneHour => "1h",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OutcomeSide {
    Up,
    Down,
}

impl OutcomeSide {
    #[must_use]
    pub const fn opposite(self) -> Self {
        match self {
            Self::Up => Self::Down,
            Self::Down => Self::Up,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConfidenceLabel {
    CoreCandidate,
    Experimental,
    ExecutionDependent,
    Disabled,
    Shadow,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PerformanceBook {
    ActiveCore,
    ShadowExperiment,
    RetiredLegacy,
    InvalidIncomplete,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PositionState {
    Flat,
    Candidate,
    Open,
    Warning,
    Protect,
    ReverseCandidate,
    Hedged,
    Locked,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DecisionAction {
    NoTrade,
    Hold,
    Candidate,
    Open,
    Warning,
    Protect,
    Reduce,
    Exit,
    Hedge,
    ReverseCandidate,
    OpenReversed,
    Locked,
    Settle,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BookLevel {
    pub price_micros: i64,
    pub size_micros: i64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SideBook {
    pub bid: BookLevel,
    pub ask: BookLevel,
    pub quote_age_ms: i64,
}

impl SideBook {
    #[must_use]
    pub fn spread_micros(self) -> Option<i64> {
        let spread = self.ask.price_micros.checked_sub(self.bid.price_micros)?;
        if spread < 0 {
            None
        } else {
            Some(spread)
        }
    }

    #[must_use]
    pub fn mid_micros(self) -> Option<i64> {
        self.bid
            .price_micros
            .checked_add(self.ask.price_micros)
            .map(|sum| sum / 2)
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MarketSecondSnapshot {
    pub condition_id: String,
    pub asset: MarketAsset,
    pub window: WindowKind,
    pub elapsed_secs: i64,
    pub observed_at_ns: i64,
    pub up: SideBook,
    pub down: SideBook,
    /// Latest observed reference level for this snapshot's asset.
    pub reference_spot_micros: i64,
    /// Reference level recorded when this window opened.
    pub window_open_micros: i64,
    pub max_quote_age_ms: i64,
    pub fee_estimate_micros: i64,
    pub slippage_estimate_micros: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenPosition {
    pub asset: MarketAsset,
    pub side: OutcomeSide,
    pub quantity_micros: i64,
    pub entry_ask_micros: i64,
    pub effective_entry_cost_micros: i64,
    pub entry_elapsed_secs: i64,
    pub entry_observed_at_ns: i64,
    /// Collateral committed at entry: quantity x effective entry cost.
    pub stake_micros: i64,
    /// P&L already booked from partial reductions on this position.
    pub realized_pnl_micros: i64,
    pub peak_bid_micros: i64,
    pub remaining_quantity_micros: i64,
    pub protective_bid_micros: Option<i64>,
    pub partials_taken: u8,
    pub reversal_used: bool,
    pub setup_id: String,
    pub book: PerformanceBook,
    pub state: PositionState,
    pub hedge_opposite_filled: bool,
    pub hedge_opposite_cost_micros: i64,
    /// Policy digest active at open (Spec 096). Absent on pre-096 checkpoints.
    #[serde(default)]
    pub configuration_digest: Option<String>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiskState {
    pub bankroll_micros: i64,
    /// Signed realized result across closed positions for this market.
    pub realized_pnl_micros: i64,
    /// Positive magnitude of realized losses, compared against the loss budget.
    pub daily_loss_micros: i64,
    pub daily_stop_count: u32,
    pub consecutive_full_stops: u32,
    pub window_paused_until_ns: i64,
    pub day_entries_blocked: bool,
}
