#![forbid(unsafe_code)]

//! Fixed-point every-second 5m/15m/1h paper strategy engine for BTC and ETH.
//!
//! This crate proposes paper decisions only. It cannot reserve capital, approve
//! risk, sign, authenticate, or submit orders.

mod accounting;
mod config;
mod engine;
mod features;
mod feed;
mod hedge;
mod journal;
mod ledger;
mod legacy;
mod lock;
mod policy;
mod position;
mod pricing;
mod recovery;
mod runtime;
mod setups;
mod sizing;
mod types;

#[cfg(test)]
mod tests;

pub use accounting::{
    mark_value_micros, notional_micros, realized_pnl_micros, settled_side,
    settlement_payout_micros, stake_micros, unrealized_pnl_micros, value_position,
    PositionValuation,
};
pub use config::{
    payout_micros, WindowConfig, CHEAP_TOKEN_REJECT_MICROS, COMPLETE_SET_MIN_LOCK_EDGE_MICROS,
    HARD_REJECT_ASK_MICROS, SEQUENTIAL_LOCK_MAX_COST_MICROS,
};
pub use engine::{EngineState, SettledWindow, TickResult};
pub use features::{build_features, points, side_ask, side_bid, FeatureHistory, SecondFeatures};
pub use feed::{
    market_to_snapshot, parse_i64_amount, strategy_markets, window_kind, FeedError, WindowsFeed,
    WindowsMarket,
};
pub use hedge::{scan_complete_set, scan_sequential_lock, CompleteSetOpportunity};
pub use journal::{DecisionRecord, SettlementOutcome};
pub use ledger::{
    BookTotals, ClosedTrade, LedgerSnapshot, OpenPositionEntry, OpenPositionView, PaperLedger,
    StrategyStats,
};
pub use legacy::{legacy_may_place_orders, legacy_registry, LegacyStrategyId};
pub use lock::{BookLock, LockError};
pub use policy::{
    late_continuation_parameter_schema, list_plugins, ActivePointer, ActivePolicy,
    LateContinuationParams, PolicyError, PolicySource, PolicyStore, SetupGateParams,
    SetupPolicyBag, WindowPolicyDocument, LATE_CONTINUATION_SETUP_ID, PLUGIN_TYPE_WINDOW_SETUP,
};
pub use recovery::{
    load_checkpoint, replay_journal, write_checkpoint, CheckpointEntry, PositionCheckpoint,
    RecoveryError, RecoverySummary,
};
pub use runtime::{count_active_core_opens, PaperRuntime, RuntimeConfig, RuntimeStatus};
pub use setups::{EntryCandidate, SetupReject};
pub use sizing::{
    plan_size, stake_risk_bps, stake_tier, CapitalState, SetupPerformance, SizePlan, StakeTier,
    MIN_SAMPLE_FOR_TIER, STAKE_BPS_AVERAGE, STAKE_BPS_GOOD, STAKE_BPS_SHADOW_CAP, STAKE_BPS_STRONG,
    STAKE_BPS_UNPROVEN, STAKE_BPS_WEAK,
};
pub use types::{
    BookLevel, ConfidenceLabel, DecisionAction, MarketAsset, MarketSecondSnapshot, OpenPosition,
    OutcomeSide, PerformanceBook, PositionState, RiskState, SideBook, WindowKind, MICROS_PER_UNIT,
    POINT_MICROS, STRATEGY_VERSION,
};
