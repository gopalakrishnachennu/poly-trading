//! Fixed-point paper book valuation (ADR 0095).
//!
//! Quantities and prices are both micros, so their product is micros-squared
//! and must be divided back down by [`MICROS_PER_UNIT`]. Every product widens
//! to `i128` first: a full-size position at a near-certain price overflows
//! `i64` in the intermediate term long before the result would.
//!
//! Positions are valued at the executable bid for their own side. The midpoint
//! is not a price anyone offered, and the entry ask is not a price anyone will
//! pay back.

use crate::types::{MarketSecondSnapshot, OutcomeSide, MICROS_PER_UNIT};
use serde::{Deserialize, Serialize};

/// Collateral value of `quantity_micros` shares priced at `price_micros`.
#[must_use]
pub fn notional_micros(quantity_micros: i64, price_micros: i64) -> i64 {
    let product = i128::from(quantity_micros) * i128::from(price_micros);
    let scaled = product / i128::from(MICROS_PER_UNIT);
    i64::try_from(scaled).unwrap_or(i64::MAX)
}

/// Stake committed to open `quantity_micros` shares at `effective_cost_micros`.
///
/// The effective cost already carries fees and slippage, so this is the full
/// collateral outlay rather than the quoted ask alone.
#[must_use]
pub fn stake_micros(quantity_micros: i64, effective_cost_micros: i64) -> i64 {
    notional_micros(quantity_micros, effective_cost_micros)
}

/// Mark-to-market value of the remaining shares at the executable bid.
#[must_use]
pub fn mark_value_micros(remaining_quantity_micros: i64, bid_micros: i64) -> i64 {
    notional_micros(remaining_quantity_micros, bid_micros)
}

/// Unrealized P&L on the remaining shares: bid value less remaining cost basis.
#[must_use]
pub fn unrealized_pnl_micros(
    remaining_quantity_micros: i64,
    bid_micros: i64,
    effective_cost_micros: i64,
) -> i64 {
    let value = mark_value_micros(remaining_quantity_micros, bid_micros);
    let basis = notional_micros(remaining_quantity_micros, effective_cost_micros);
    value.saturating_sub(basis)
}

/// Realized P&L from closing `quantity_micros` shares at `exit_price_micros`.
///
/// Used for partial reductions, full exits, and settlement alike; settlement
/// simply passes a payout of `1.0` or `0.0` as the exit price.
#[must_use]
pub fn realized_pnl_micros(
    quantity_micros: i64,
    exit_price_micros: i64,
    effective_cost_micros: i64,
) -> i64 {
    let proceeds = notional_micros(quantity_micros, exit_price_micros);
    let basis = notional_micros(quantity_micros, effective_cost_micros);
    proceeds.saturating_sub(basis)
}

/// Terminal payout per share for a settled binary outcome.
#[must_use]
pub const fn settlement_payout_micros(won: bool) -> i64 {
    if won {
        MICROS_PER_UNIT
    } else {
        0
    }
}

/// Which side a window resolved to, from the observed reference level.
///
/// This is the paper mark described in ADR 0095, not the authoritative
/// resolver. A flat close resolves DOWN, matching the "strictly above the open"
/// convention these UP/DOWN windows are written against.
#[must_use]
pub const fn settled_side(window_open_micros: i64, final_reference_micros: i64) -> OutcomeSide {
    if final_reference_micros > window_open_micros {
        OutcomeSide::Up
    } else {
        OutcomeSide::Down
    }
}

/// Live valuation of one open position against the current book.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PositionValuation {
    pub bid_micros: i64,
    pub stake_micros: i64,
    pub mark_value_micros: i64,
    pub unrealized_pnl_micros: i64,
    pub realized_pnl_micros: i64,
}

impl PositionValuation {
    /// Total P&L booked and marked so far.
    #[must_use]
    pub const fn total_pnl_micros(&self) -> i64 {
        self.realized_pnl_micros
            .saturating_add(self.unrealized_pnl_micros)
    }
}

/// Value an open position against the current snapshot's executable bid.
#[must_use]
pub fn value_position(
    snapshot: &MarketSecondSnapshot,
    position: &crate::types::OpenPosition,
) -> PositionValuation {
    let bid = crate::features::side_bid(snapshot, position.side);
    PositionValuation {
        bid_micros: bid,
        stake_micros: position.stake_micros,
        mark_value_micros: mark_value_micros(position.remaining_quantity_micros, bid),
        unrealized_pnl_micros: unrealized_pnl_micros(
            position.remaining_quantity_micros,
            bid,
            position.effective_entry_cost_micros,
        ),
        realized_pnl_micros: position.realized_pnl_micros,
    }
}
