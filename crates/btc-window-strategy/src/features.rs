//! Every-second feature construction.

use crate::types::{MarketSecondSnapshot, OpenPosition, OutcomeSide, WindowKind, POINT_MICROS};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SecondFeatures {
    pub favourite: OutcomeSide,
    pub confidence_micros: i64,
    pub favourite_ask_micros: i64,
    pub favourite_bid_micros: i64,
    pub favourite_spread_micros: i64,
    pub up_mid_micros: i64,
    pub down_mid_micros: i64,
    pub persistence_secs: i64,
    pub flips: u32,
    pub mom_short_micros: i64,
    pub mom_med_micros: i64,
    pub mom_long_micros: i64,
    pub accel_micros: i64,
    pub book_imbalance_bps: i64,
    pub reference_return_bps: i64,
    pub reference_agrees_with_favourite: bool,
    pub top_depth_ok: bool,
    pub quote_fresh: bool,
    pub position_pnl_micros: Option<i64>,
    pub position_drawdown_from_peak_micros: Option<i64>,
    pub confidence_delta_60s_micros: i64,
}

#[derive(Clone, Debug, Default)]
pub struct FeatureHistory {
    pub favourite_side: Option<OutcomeSide>,
    pub favourite_since_elapsed: i64,
    pub flips: u32,
    pub mids: Vec<(i64, i64)>, // (elapsed, favourite mid)
    pub confidences: Vec<(i64, i64)>,
}

impl FeatureHistory {
    pub fn observe(&mut self, elapsed: i64, favourite: OutcomeSide, mid: i64, confidence: i64) {
        match self.favourite_side {
            Some(prev) if prev == favourite => {}
            Some(_) => {
                self.flips = self.flips.saturating_add(1);
                self.favourite_side = Some(favourite);
                self.favourite_since_elapsed = elapsed;
            }
            None => {
                self.favourite_side = Some(favourite);
                self.favourite_since_elapsed = elapsed;
            }
        }
        self.mids.push((elapsed, mid));
        self.confidences.push((elapsed, confidence));
        if self.mids.len() > 4_000 {
            self.mids.drain(0..self.mids.len() - 3_600);
        }
        if self.confidences.len() > 4_000 {
            self.confidences.drain(0..self.confidences.len() - 3_600);
        }
    }

    #[must_use]
    pub fn persistence_secs(&self, elapsed: i64) -> i64 {
        elapsed.saturating_sub(self.favourite_since_elapsed).max(0)
    }

    #[must_use]
    fn mid_change(&self, elapsed: i64, lookback: i64) -> i64 {
        let target = elapsed.saturating_sub(lookback);
        let current = self.mids.last().map_or(0, |(_, m)| *m);
        let prior = self
            .mids
            .iter()
            .rev()
            .find(|(e, _)| *e <= target)
            .map_or(current, |(_, m)| *m);
        current.saturating_sub(prior)
    }

    #[must_use]
    pub fn confidence_delta(&self, elapsed: i64, lookback: i64) -> i64 {
        let target = elapsed.saturating_sub(lookback);
        let current = self.confidences.last().map_or(0, |(_, c)| *c);
        let prior = self
            .confidences
            .iter()
            .rev()
            .find(|(e, _)| *e <= target)
            .map_or(current, |(_, c)| *c);
        current.saturating_sub(prior)
    }

    #[must_use]
    pub fn momentum_triple(&self, window: WindowKind, elapsed: i64) -> (i64, i64, i64) {
        match window {
            WindowKind::FiveMinute => (
                self.mid_change(elapsed, 5),
                self.mid_change(elapsed, 15),
                self.mid_change(elapsed, 30),
            ),
            WindowKind::FifteenMinute => (
                self.mid_change(elapsed, 15),
                self.mid_change(elapsed, 60),
                self.mid_change(elapsed, 120),
            ),
            WindowKind::OneHour => (
                self.mid_change(elapsed, 60),
                self.mid_change(elapsed, 300),
                self.mid_change(elapsed, 600),
            ),
        }
    }
}

#[must_use]
pub fn build_features(
    snapshot: &MarketSecondSnapshot,
    history: &mut FeatureHistory,
    position: Option<&OpenPosition>,
) -> Option<SecondFeatures> {
    let up_mid = snapshot.up.mid_micros()?;
    let down_mid = snapshot.down.mid_micros()?;
    let up_spread = snapshot.up.spread_micros()?;
    let down_spread = snapshot.down.spread_micros()?;

    let (favourite, confidence_micros, favourite_ask, favourite_bid, favourite_spread) =
        if up_mid >= down_mid {
            (
                OutcomeSide::Up,
                up_mid,
                snapshot.up.ask.price_micros,
                snapshot.up.bid.price_micros,
                up_spread,
            )
        } else {
            (
                OutcomeSide::Down,
                down_mid,
                snapshot.down.ask.price_micros,
                snapshot.down.bid.price_micros,
                down_spread,
            )
        };

    history.observe(
        snapshot.elapsed_secs,
        favourite,
        confidence_micros,
        confidence_micros,
    );
    let (mom_short, mom_med, mom_long) =
        history.momentum_triple(snapshot.window, snapshot.elapsed_secs);
    let prev_short = history.mid_change(
        snapshot
            .elapsed_secs
            .saturating_sub(mom_lookback(snapshot.window)),
        mom_lookback(snapshot.window),
    );
    let accel = mom_short.saturating_sub(prev_short);

    let up_bid_sz = snapshot.up.bid.size_micros;
    let up_ask_sz = snapshot.up.ask.size_micros;
    let down_bid_sz = snapshot.down.bid.size_micros;
    let down_ask_sz = snapshot.down.ask.size_micros;
    let imb_num = (up_bid_sz + down_ask_sz).saturating_sub(down_bid_sz + up_ask_sz);
    let imb_den = (up_bid_sz + up_ask_sz + down_bid_sz + down_ask_sz).max(1);
    let book_imbalance_bps = imb_num.saturating_mul(10_000) / imb_den;

    let reference_return_bps = if snapshot.window_open_micros > 0 {
        snapshot
            .reference_spot_micros
            .saturating_sub(snapshot.window_open_micros)
            .saturating_mul(10_000)
            / snapshot.window_open_micros
    } else {
        0
    };
    let reference_agrees = match favourite {
        OutcomeSide::Up => reference_return_bps >= 0,
        OutcomeSide::Down => reference_return_bps <= 0,
    };

    let quote_fresh = snapshot.up.quote_age_ms <= snapshot.max_quote_age_ms
        && snapshot.down.quote_age_ms <= snapshot.max_quote_age_ms;
    let top_depth_ok = snapshot.up.bid.size_micros > 0
        && snapshot.up.ask.size_micros > 0
        && snapshot.down.bid.size_micros > 0
        && snapshot.down.ask.size_micros > 0;

    let (position_pnl_micros, position_drawdown_from_peak_micros) = if let Some(pos) = position {
        let bid = side_bid(snapshot, pos.side);
        let pnl = bid.saturating_sub(pos.effective_entry_cost_micros);
        let dd = pos.peak_bid_micros.saturating_sub(bid);
        (Some(pnl), Some(dd))
    } else {
        (None, None)
    };

    Some(SecondFeatures {
        favourite,
        confidence_micros,
        favourite_ask_micros: favourite_ask,
        favourite_bid_micros: favourite_bid,
        favourite_spread_micros: favourite_spread,
        up_mid_micros: up_mid,
        down_mid_micros: down_mid,
        persistence_secs: history.persistence_secs(snapshot.elapsed_secs),
        flips: history.flips,
        mom_short_micros: mom_short,
        mom_med_micros: mom_med,
        mom_long_micros: mom_long,
        accel_micros: accel,
        book_imbalance_bps,
        reference_return_bps,
        reference_agrees_with_favourite: reference_agrees,
        top_depth_ok,
        quote_fresh,
        position_pnl_micros,
        position_drawdown_from_peak_micros,
        confidence_delta_60s_micros: history.confidence_delta(snapshot.elapsed_secs, 60),
    })
}

#[must_use]
pub fn side_ask(snapshot: &MarketSecondSnapshot, side: OutcomeSide) -> i64 {
    match side {
        OutcomeSide::Up => snapshot.up.ask.price_micros,
        OutcomeSide::Down => snapshot.down.ask.price_micros,
    }
}

#[must_use]
pub fn side_bid(snapshot: &MarketSecondSnapshot, side: OutcomeSide) -> i64 {
    match side {
        OutcomeSide::Up => snapshot.up.bid.price_micros,
        OutcomeSide::Down => snapshot.down.bid.price_micros,
    }
}

#[must_use]
pub fn side_spread(snapshot: &MarketSecondSnapshot, side: OutcomeSide) -> Option<i64> {
    match side {
        OutcomeSide::Up => snapshot.up.spread_micros(),
        OutcomeSide::Down => snapshot.down.spread_micros(),
    }
}

#[must_use]
fn mom_lookback(window: WindowKind) -> i64 {
    match window {
        WindowKind::FiveMinute => 5,
        WindowKind::FifteenMinute => 15,
        WindowKind::OneHour => 60,
    }
}

/// Convert points to micros helper used by tests.
#[must_use]
pub const fn points(n: i64) -> i64 {
    n * POINT_MICROS
}
