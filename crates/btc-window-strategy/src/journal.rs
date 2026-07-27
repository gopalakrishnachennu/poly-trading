//! Decision journal records and performance-book separation.

use crate::accounting::{mark_value_micros, stake_micros, unrealized_pnl_micros};
use crate::features::side_bid;
use crate::setups::{EntryCandidate, SetupReject};
use crate::sizing::SizePlan;
use crate::types::{
    DecisionAction, MarketAsset, MarketSecondSnapshot, PerformanceBook, PositionState,
    STRATEGY_VERSION,
};
use serde::{Deserialize, Serialize};

/// Result of a settled paper window.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SettlementOutcome {
    Win,
    Loss,
}

/// One journalled decision second.
///
/// Fields added after Spec 090 carry `#[serde(default)]` so that journals
/// written by the earlier engine still deserialize during recovery. A missing
/// accounting field reads as absent, never as a fabricated zero result.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[allow(clippy::struct_field_names)] // Explicit micros units at the financial boundary.
pub struct DecisionRecord {
    pub strategy_version: String,
    pub condition_id: String,
    #[serde(default = "default_asset")]
    pub asset: MarketAsset,
    pub window: String,
    pub elapsed_secs: i64,
    pub observed_at_ns: i64,
    pub action: DecisionAction,
    pub reason: String,
    pub rejection_reasons: Vec<String>,
    pub setup_id: Option<String>,
    pub side: Option<String>,
    pub ask_micros: Option<i64>,
    pub bid_micros: Option<i64>,
    pub edge_micros: Option<i64>,
    pub effective_cost_micros: Option<i64>,
    pub quantity_micros: Option<i64>,
    /// Collateral committed by this position, in micros.
    #[serde(default)]
    pub stake_micros: Option<i64>,
    /// Remaining shares valued at the executable bid.
    #[serde(default)]
    pub mark_value_micros: Option<i64>,
    #[serde(default)]
    pub unrealized_pnl_micros: Option<i64>,
    #[serde(default)]
    pub realized_pnl_micros: Option<i64>,
    /// Seconds held since entry, present once a position exists.
    #[serde(default)]
    pub hold_secs: Option<i64>,
    #[serde(default)]
    pub outcome: Option<SettlementOutcome>,
    /// Present on `Settle` records; see ADR 0095.
    #[serde(default)]
    pub settlement_source: Option<String>,
    /// Adaptive stake tier at entry (Spec 093); absent on pre-sizing journals.
    #[serde(default)]
    pub stake_tier: Option<String>,
    /// Risk budget in bps of tradable bankroll used to size this entry.
    #[serde(default)]
    pub risk_bps: Option<i64>,
    /// Active window policy digest at decision time (Spec 096).
    #[serde(default)]
    pub configuration_digest: Option<String>,
    pub position_state: PositionState,
    pub book: PerformanceBook,
    pub paper_only: bool,
    pub real_money: bool,
}

/// Journals written before Spec 091 were BTC-only by construction.
const fn default_asset() -> MarketAsset {
    MarketAsset::Btc
}

impl DecisionRecord {
    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        let payload = serde_json::to_vec(self).unwrap_or_default();
        *blake3::hash(&payload).as_bytes()
    }
}

#[must_use]
pub fn no_trade(
    snapshot: &MarketSecondSnapshot,
    window: &str,
    reason: &str,
    rejects: &[SetupReject],
    state: PositionState,
    book: PerformanceBook,
) -> DecisionRecord {
    DecisionRecord {
        strategy_version: STRATEGY_VERSION.into(),
        condition_id: snapshot.condition_id.clone(),
        asset: snapshot.asset,
        window: window.into(),
        elapsed_secs: snapshot.elapsed_secs,
        observed_at_ns: snapshot.observed_at_ns,
        action: DecisionAction::NoTrade,
        reason: reason.into(),
        rejection_reasons: rejects
            .iter()
            .map(|r| format!("{}:{}", r.setup_id, r.reason))
            .collect(),
        setup_id: None,
        side: None,
        ask_micros: None,
        bid_micros: None,
        edge_micros: None,
        effective_cost_micros: None,
        quantity_micros: None,
        stake_micros: None,
        mark_value_micros: None,
        unrealized_pnl_micros: None,
        realized_pnl_micros: None,
        hold_secs: None,
        outcome: None,
        settlement_source: None,
        stake_tier: None,
        risk_bps: None,
        configuration_digest: None,
        position_state: state,
        book,
        paper_only: true,
        real_money: false,
    }
}

#[must_use]
pub fn from_candidate(
    snapshot: &MarketSecondSnapshot,
    window: &str,
    candidate: &EntryCandidate,
    size: &SizePlan,
    action: DecisionAction,
    reason: &str,
    configuration_digest: Option<String>,
) -> DecisionRecord {
    let quantity_micros = size.quantity_micros;
    let effective_cost = candidate.pricing.effective_entry_cost_micros;
    DecisionRecord {
        strategy_version: STRATEGY_VERSION.into(),
        condition_id: snapshot.condition_id.clone(),
        asset: snapshot.asset,
        window: window.into(),
        elapsed_secs: snapshot.elapsed_secs,
        observed_at_ns: snapshot.observed_at_ns,
        action,
        reason: reason.into(),
        rejection_reasons: Vec::new(),
        setup_id: Some(candidate.setup_id.clone()),
        side: Some(format!("{:?}", candidate.side).to_ascii_lowercase()),
        ask_micros: Some(candidate.pricing.ask_micros),
        bid_micros: Some(side_bid(snapshot, candidate.side)),
        edge_micros: Some(candidate.pricing.edge_micros),
        effective_cost_micros: Some(effective_cost),
        quantity_micros: Some(quantity_micros),
        stake_micros: Some(stake_micros(quantity_micros, effective_cost)),
        mark_value_micros: Some(mark_value_micros(
            quantity_micros,
            side_bid(snapshot, candidate.side),
        )),
        unrealized_pnl_micros: Some(unrealized_pnl_micros(
            quantity_micros,
            side_bid(snapshot, candidate.side),
            effective_cost,
        )),
        realized_pnl_micros: Some(0),
        hold_secs: Some(0),
        outcome: None,
        settlement_source: None,
        stake_tier: Some(size.stake_tier.label().to_owned()),
        risk_bps: Some(size.risk_bps),
        configuration_digest,
        position_state: PositionState::Open,
        book: candidate.book,
        paper_only: true,
        real_money: false,
    }
}
