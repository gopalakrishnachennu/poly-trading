//! Open-position management: protect, reduce, exit, reverse-candidate.

use crate::accounting::realized_pnl_micros;
use crate::config::WindowConfig;
use crate::features::{side_bid, SecondFeatures};
use crate::hedge::scan_sequential_lock;
use crate::types::{
    DecisionAction, MarketSecondSnapshot, OpenPosition, OutcomeSide, PositionState, WindowKind,
};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PositionAction {
    pub action: DecisionAction,
    pub reason: String,
    pub reduce_quantity_micros: i64,
    pub exit_all: bool,
    pub enter_reverse_candidate: bool,
    pub apply_hedge: bool,
    pub new_state: PositionState,
    pub protective_bid_micros: Option<i64>,
}

#[allow(clippy::too_many_lines)]
#[must_use]
pub fn manage_open_position(
    snapshot: &MarketSecondSnapshot,
    features: &SecondFeatures,
    position: &OpenPosition,
    cfg: &WindowConfig,
    opposite_flip_secs: i64,
) -> PositionAction {
    if position.state == PositionState::Locked || position.hedge_opposite_filled {
        return PositionAction {
            action: DecisionAction::Locked,
            reason: "LOCKED_HOLD_TO_SETTLEMENT".into(),
            reduce_quantity_micros: 0,
            exit_all: false,
            enter_reverse_candidate: false,
            apply_hedge: false,
            new_state: PositionState::Locked,
            protective_bid_micros: position.protective_bid_micros,
        };
    }

    if let Some(_lock) = scan_sequential_lock(snapshot, position) {
        return PositionAction {
            action: DecisionAction::Hedge,
            reason: "SEQUENTIAL_LOCK_EXECUTABLE".into(),
            reduce_quantity_micros: 0,
            exit_all: false,
            enter_reverse_candidate: false,
            apply_hedge: true,
            new_state: PositionState::Locked,
            protective_bid_micros: position.protective_bid_micros,
        };
    }

    let bid = side_bid(snapshot, position.side);
    let peak = position.peak_bid_micros.max(bid);
    let pnl = bid.saturating_sub(position.effective_entry_cost_micros);
    let fall_from_entry = position
        .effective_entry_cost_micros
        .saturating_sub(bid)
        .max(0);

    // Profit protection / partials.
    let mut protective = position.protective_bid_micros;
    let mut reduce_qty = 0_i64;
    let mut state = position.state;

    if pnl >= cfg.profit_protect_micros {
        protective = Some(position.effective_entry_cost_micros);
        state = PositionState::Protect;
    }
    if pnl >= cfg.profit_partial1_micros && position.partials_taken < 1 {
        reduce_qty = position.remaining_quantity_micros / 4;
        state = PositionState::Protect;
    }
    if pnl >= cfg.profit_partial2_micros && position.partials_taken < 2 {
        let extra = position
            .remaining_quantity_micros
            .saturating_sub(reduce_qty)
            / 2;
        reduce_qty = reduce_qty.saturating_add(extra);
        state = PositionState::Protect;
    }
    if state == PositionState::Protect {
        if let Some(peak_trail) = peak.checked_sub(cfg.trail_lo_micros) {
            protective = Some(protective.map_or(peak_trail, |p| p.max(peak_trail)));
        }
    }

    if let Some(stop) = protective {
        if bid <= stop && pnl > 0 {
            return PositionAction {
                action: DecisionAction::Exit,
                reason: "TRAILING_OR_PROTECT_STOP".into(),
                reduce_quantity_micros: position.remaining_quantity_micros,
                exit_all: true,
                enter_reverse_candidate: false,
                apply_hedge: false,
                new_state: PositionState::Flat,
                protective_bid_micros: protective,
            };
        }
    }

    // Emergency price stop.
    let emergency = cfg.emergency_stop_lo_micros;
    if fall_from_entry >= emergency {
        let momentum_reversed = match position.side {
            OutcomeSide::Up => features.mom_med_micros < 0 && features.mom_long_micros < 0,
            OutcomeSide::Down => features.mom_med_micros > 0 && features.mom_long_micros > 0,
        };
        let force = fall_from_entry >= cfg.emergency_stop_hi_micros || momentum_reversed;
        if force {
            return PositionAction {
                action: DecisionAction::Exit,
                reason: "EMERGENCY_PRICE_STOP".into(),
                reduce_quantity_micros: position.remaining_quantity_micros,
                exit_all: true,
                enter_reverse_candidate: false,
                apply_hedge: false,
                new_state: PositionState::Flat,
                protective_bid_micros: protective,
            };
        }
    }

    // Signal / favourite invalidation.
    let flipped = features.favourite != position.side;
    let reference_crossed = match position.side {
        OutcomeSide::Up => features.reference_return_bps < 0,
        OutcomeSide::Down => features.reference_return_bps > 0,
    };
    let late_5m = snapshot.window == WindowKind::FiveMinute && snapshot.elapsed_secs >= 240;

    if flipped && opposite_flip_secs >= cfg.reverse_confirm_secs && reference_crossed {
        return PositionAction {
            action: DecisionAction::Exit,
            reason: "THESIS_INVALIDATED_FLIP".into(),
            reduce_quantity_micros: position.remaining_quantity_micros,
            exit_all: true,
            enter_reverse_candidate: !position.reversal_used,
            apply_hedge: false,
            new_state: if position.reversal_used {
                PositionState::Flat
            } else {
                PositionState::ReverseCandidate
            },
            protective_bid_micros: protective,
        };
    }
    if late_5m && flipped {
        return PositionAction {
            action: DecisionAction::Exit,
            reason: "LATE_WINDOW_FLIP_EXIT".into(),
            reduce_quantity_micros: position.remaining_quantity_micros,
            exit_all: true,
            enter_reverse_candidate: false,
            apply_hedge: false,
            new_state: PositionState::Flat,
            protective_bid_micros: protective,
        };
    }

    // Time stop: no favourable move within 30s and signal weakens (5m).
    if snapshot.window == WindowKind::FiveMinute {
        let time_in = snapshot
            .elapsed_secs
            .saturating_sub(position.entry_elapsed_secs);
        if time_in >= 30 && pnl <= 0 && thesis_weakening(features, position.side) {
            return PositionAction {
                action: DecisionAction::Exit,
                reason: "TIME_STOP".into(),
                reduce_quantity_micros: position.remaining_quantity_micros,
                exit_all: true,
                enter_reverse_candidate: false,
                apply_hedge: false,
                new_state: PositionState::Flat,
                protective_bid_micros: protective,
            };
        }
    }

    if thesis_weakening(features, position.side) {
        if reduce_qty > 0 {
            return PositionAction {
                action: DecisionAction::Reduce,
                reason: "WARNING_WITH_PARTIAL".into(),
                reduce_quantity_micros: reduce_qty,
                exit_all: false,
                enter_reverse_candidate: false,
                apply_hedge: false,
                new_state: PositionState::Warning,
                protective_bid_micros: protective,
            };
        }
        return PositionAction {
            action: DecisionAction::Warning,
            reason: "THESIS_WEAKENING".into(),
            reduce_quantity_micros: 0,
            exit_all: false,
            enter_reverse_candidate: false,
            apply_hedge: false,
            new_state: PositionState::Warning,
            protective_bid_micros: protective,
        };
    }

    if reduce_qty > 0 {
        return PositionAction {
            action: DecisionAction::Reduce,
            reason: "PROFIT_PARTIAL".into(),
            reduce_quantity_micros: reduce_qty,
            exit_all: false,
            enter_reverse_candidate: false,
            apply_hedge: false,
            new_state: state,
            protective_bid_micros: protective,
        };
    }

    PositionAction {
        action: if state == PositionState::Protect {
            DecisionAction::Protect
        } else {
            DecisionAction::Hold
        },
        reason: "THESIS_INTACT".into(),
        reduce_quantity_micros: 0,
        exit_all: false,
        enter_reverse_candidate: false,
        apply_hedge: false,
        new_state: if state == PositionState::Protect {
            PositionState::Protect
        } else {
            PositionState::Open
        },
        protective_bid_micros: protective,
    }
}

#[must_use]
fn thesis_weakening(features: &SecondFeatures, side: OutcomeSide) -> bool {
    let mom_bad = match side {
        OutcomeSide::Up => features.mom_med_micros < 0,
        OutcomeSide::Down => features.mom_med_micros > 0,
    };
    let fav_bad = features.favourite != side;
    let reference_bad = !match side {
        OutcomeSide::Up => features.reference_return_bps >= 0,
        OutcomeSide::Down => features.reference_return_bps <= 0,
    };
    // Two signals weaken.
    [mom_bad, fav_bad, reference_bad]
        .iter()
        .filter(|&&x| x)
        .count()
        >= 2
}

/// Apply an action to the position and book any P&L it realizes.
///
/// Reduced shares are sold into the executable bid, so the proceeds are known
/// at this point and are realized immediately rather than left marked.
pub fn apply_position_action(position: &mut OpenPosition, action: &PositionAction, bid: i64) {
    position.peak_bid_micros = position.peak_bid_micros.max(bid);
    position.protective_bid_micros = action.protective_bid_micros;
    position.state = action.new_state;
    if action.apply_hedge {
        position.hedge_opposite_filled = true;
        position.state = PositionState::Locked;
    }
    if action.reduce_quantity_micros > 0 {
        let closed = action
            .reduce_quantity_micros
            .min(position.remaining_quantity_micros);
        position.realized_pnl_micros =
            position
                .realized_pnl_micros
                .saturating_add(realized_pnl_micros(
                    closed,
                    bid,
                    position.effective_entry_cost_micros,
                ));
        position.remaining_quantity_micros =
            position.remaining_quantity_micros.saturating_sub(closed);
        if position.partials_taken < 2 {
            position.partials_taken = position.partials_taken.saturating_add(1);
        }
    }
    if action.exit_all || position.remaining_quantity_micros == 0 {
        position.remaining_quantity_micros = 0;
        if !action.enter_reverse_candidate {
            position.state = PositionState::Flat;
        }
    }
}
