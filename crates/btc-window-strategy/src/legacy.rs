//! Legacy strategies are DISABLED as order triggers.

use crate::types::ConfidenceLabel;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LegacyStrategyId {
    BookImbalance,
    FavoriteProbability,
    FairValueModel,
    MultiHorizonMomentum,
    FixedThresholdIntradayRoundTrip,
    MomentumStandalone,
    VolRegime,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LegacyStrategyStatus {
    pub id: LegacyStrategyId,
    pub label: ConfidenceLabel,
    pub may_place_orders: bool,
    pub retain_as_feature: bool,
    pub reason: &'static str,
}

#[must_use]
pub fn legacy_registry() -> [LegacyStrategyStatus; 7] {
    [
        LegacyStrategyStatus {
            id: LegacyStrategyId::BookImbalance,
            label: ConfidenceLabel::Disabled,
            may_place_orders: false,
            retain_as_feature: true,
            reason: "May be cancelled/spoof-like; only marginally positive; retain as feature",
        },
        LegacyStrategyStatus {
            id: LegacyStrategyId::FavoriteProbability,
            label: ConfidenceLabel::Disabled,
            may_place_orders: false,
            retain_as_feature: false,
            reason: "High win-rate trap with poor payoff asymmetry",
        },
        LegacyStrategyStatus {
            id: LegacyStrategyId::FairValueModel,
            label: ConfidenceLabel::Disabled,
            may_place_orders: false,
            retain_as_feature: false,
            reason: "Likely miscalibration and premature Kelly; pending rebuild",
        },
        LegacyStrategyStatus {
            id: LegacyStrategyId::MultiHorizonMomentum,
            label: ConfidenceLabel::Disabled,
            may_place_orders: false,
            retain_as_feature: false,
            reason: "Too few trades; entered expensive contracts too late",
        },
        LegacyStrategyStatus {
            id: LegacyStrategyId::FixedThresholdIntradayRoundTrip,
            label: ConfidenceLabel::Disabled,
            may_place_orders: false,
            retain_as_feature: false,
            reason: "Negative out-of-sample; cheap is not underpriced",
        },
        LegacyStrategyStatus {
            id: LegacyStrategyId::MomentumStandalone,
            label: ConfidenceLabel::Disabled,
            may_place_orders: false,
            retain_as_feature: false,
            reason: "No recorded trades in consolidated history; disabled as trigger",
        },
        LegacyStrategyStatus {
            id: LegacyStrategyId::VolRegime,
            label: ConfidenceLabel::Disabled,
            may_place_orders: false,
            retain_as_feature: false,
            reason: "No recorded trades in consolidated history; disabled as trigger",
        },
    ]
}

#[must_use]
pub fn legacy_may_place_orders(id: LegacyStrategyId) -> bool {
    legacy_registry()
        .iter()
        .find(|s| s.id == id)
        .is_some_and(|s| s.may_place_orders)
}
