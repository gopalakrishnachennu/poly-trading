//! Complete-set and sequential-hedge scanners.

use crate::config::{COMPLETE_SET_MIN_LOCK_EDGE_MICROS, SEQUENTIAL_LOCK_MAX_COST_MICROS};
use crate::features::side_ask;
use crate::types::{MarketSecondSnapshot, OpenPosition, OutcomeSide, MICROS_PER_UNIT};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CompleteSetOpportunity {
    pub up_ask_micros: i64,
    pub down_ask_micros: i64,
    pub combined_cost_micros: i64,
    pub locked_edge_micros: i64,
    pub quantity_micros: i64,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SequentialLockOpportunity {
    pub opposite: OutcomeSide,
    pub opposite_ask_micros: i64,
    pub total_cost_micros: i64,
    pub quantity_micros: i64,
}

#[must_use]
pub fn scan_complete_set(
    snapshot: &MarketSecondSnapshot,
    quantity_micros: i64,
) -> Option<CompleteSetOpportunity> {
    if quantity_micros <= 0 {
        return None;
    }
    if snapshot.up.quote_age_ms > snapshot.max_quote_age_ms
        || snapshot.down.quote_age_ms > snapshot.max_quote_age_ms
    {
        return None;
    }
    if snapshot.up.ask.size_micros < quantity_micros
        || snapshot.down.ask.size_micros < quantity_micros
    {
        return None;
    }
    let up = snapshot.up.ask.price_micros;
    let down = snapshot.down.ask.price_micros;
    let fees = snapshot
        .fee_estimate_micros
        .saturating_mul(2)
        .saturating_add(snapshot.slippage_estimate_micros.saturating_mul(2));
    let combined = up.saturating_add(down).saturating_add(fees);
    let locked_edge = MICROS_PER_UNIT.saturating_sub(combined);
    if locked_edge < COMPLETE_SET_MIN_LOCK_EDGE_MICROS {
        return None;
    }
    Some(CompleteSetOpportunity {
        up_ask_micros: up,
        down_ask_micros: down,
        combined_cost_micros: combined,
        locked_edge_micros: locked_edge,
        quantity_micros,
    })
}

#[must_use]
pub fn scan_sequential_lock(
    snapshot: &MarketSecondSnapshot,
    position: &OpenPosition,
) -> Option<SequentialLockOpportunity> {
    if position.hedge_opposite_filled || position.remaining_quantity_micros <= 0 {
        return None;
    }
    let opposite = position.side.opposite();
    let opposite_ask = side_ask(snapshot, opposite);
    if snapshot.up.quote_age_ms > snapshot.max_quote_age_ms
        || snapshot.down.quote_age_ms > snapshot.max_quote_age_ms
    {
        return None;
    }
    let depth = match opposite {
        OutcomeSide::Up => snapshot.up.ask.size_micros,
        OutcomeSide::Down => snapshot.down.ask.size_micros,
    };
    if depth < position.remaining_quantity_micros {
        return None;
    }
    let second_cost = opposite_ask
        .saturating_add(snapshot.fee_estimate_micros)
        .saturating_add(snapshot.slippage_estimate_micros);
    let total = position
        .effective_entry_cost_micros
        .saturating_add(second_cost);
    if total > SEQUENTIAL_LOCK_MAX_COST_MICROS {
        return None;
    }
    Some(SequentialLockOpportunity {
        opposite,
        opposite_ask_micros: opposite_ask,
        total_cost_micros: total,
        quantity_micros: position.remaining_quantity_micros,
    })
}
