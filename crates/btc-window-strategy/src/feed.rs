//! Convert unified windows observation payloads into engine snapshots.

use crate::types::{BookLevel, MarketAsset, MarketSecondSnapshot, SideBook, WindowKind};
use serde::Deserialize;
use thiserror::Error;

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum FeedError {
    #[error("unsupported or missing window")]
    BadWindow,
    #[error("missing executable bid/ask")]
    MissingBook,
    #[error("invalid fixed-point amount")]
    BadAmount,
    #[error("asset has no configured window strategy")]
    UnsupportedAsset,
    #[error("invalid timing")]
    BadTiming,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WindowsFeed {
    pub markets: Vec<WindowsMarket>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct WindowsMarket {
    pub asset: String,
    pub window: String,
    pub cond: String,
    pub start_ms: i64,
    pub end_ms: i64,
    pub t_ms: i64,
    pub reference_micros: Option<String>,
    pub target_micros: Option<String>,
    pub up_book: FeedBook,
    pub down_book: FeedBook,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FeedBook {
    pub best_bid_micros: String,
    pub best_ask_micros: String,
    #[serde(default)]
    pub bids: Vec<FeedLevel>,
    #[serde(default)]
    pub asks: Vec<FeedLevel>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct FeedLevel {
    pub price_micros: String,
    pub quantity_micros: String,
}

/// Parse a decimal integer micros string.
///
/// # Errors
///
/// Returns [`FeedError::BadAmount`] when the string is not a signed integer.
pub fn parse_i64_amount(raw: &str) -> Result<i64, FeedError> {
    raw.parse::<i64>().map_err(|_| FeedError::BadAmount)
}

/// Map feed window labels to engine window kinds.
///
/// # Errors
///
/// Returns [`FeedError::BadWindow`] for unsupported labels.
pub fn window_kind(label: &str) -> Result<WindowKind, FeedError> {
    match label {
        "5m" => Ok(WindowKind::FiveMinute),
        "15m" => Ok(WindowKind::FifteenMinute),
        "1h" => Ok(WindowKind::OneHour),
        _ => Err(FeedError::BadWindow),
    }
}

fn top_level(book: &FeedBook, want_bid: bool) -> Result<BookLevel, FeedError> {
    let (levels, best_price) = if want_bid {
        (&book.bids, &book.best_bid_micros)
    } else {
        (&book.asks, &book.best_ask_micros)
    };
    if let Some(level) = levels.first() {
        let price = parse_i64_amount(&level.price_micros)?;
        let size = parse_i64_amount(&level.quantity_micros)?;
        if price <= 0 || size <= 0 {
            return Err(FeedError::MissingBook);
        }
        return Ok(BookLevel {
            price_micros: price,
            size_micros: size,
        });
    }
    let price = parse_i64_amount(best_price)?;
    if price <= 0 {
        return Err(FeedError::MissingBook);
    }
    // Depth missing: reject rather than inventing a midpoint fill size.
    Err(FeedError::MissingBook)
}

fn side_book(book: &FeedBook, quote_age_ms: i64) -> Result<SideBook, FeedError> {
    let bid = top_level(book, true)?;
    let ask = top_level(book, false)?;
    if ask.price_micros < bid.price_micros {
        return Err(FeedError::MissingBook);
    }
    Ok(SideBook {
        bid,
        ask,
        quote_age_ms,
    })
}

/// Build one engine snapshot from a windows market row.
///
/// # Errors
///
/// Returns [`FeedError`] when the asset has no configured window strategy,
/// books are incomplete, timing is invalid, or amounts cannot be parsed as
/// integers.
pub fn market_to_snapshot(
    market: &WindowsMarket,
    observed_at_ns: i64,
    max_quote_age_ms: i64,
    fee_estimate_micros: i64,
    slippage_estimate_micros: i64,
    reference_spot_override_micros: Option<i64>,
) -> Result<MarketSecondSnapshot, FeedError> {
    let asset = MarketAsset::from_symbol(&market.asset).ok_or(FeedError::UnsupportedAsset)?;
    let window = window_kind(&market.window)?;
    if market.end_ms <= market.start_ms || market.t_ms < market.start_ms {
        return Err(FeedError::BadTiming);
    }
    let elapsed_secs = ((market.t_ms - market.start_ms) / 1_000).clamp(0, window.duration_secs());
    let observe_ms = observed_at_ns / 1_000_000;
    let age = observe_ms.saturating_sub(market.t_ms).max(0);

    let up = side_book(&market.up_book, age)?;
    let down = side_book(&market.down_book, age)?;

    let reference = market
        .reference_micros
        .as_deref()
        .map(parse_i64_amount)
        .transpose()?
        .or(reference_spot_override_micros)
        .unwrap_or(0);
    let spot = reference_spot_override_micros.unwrap_or(reference);
    let open = market
        .target_micros
        .as_deref()
        .map(parse_i64_amount)
        .transpose()?
        .filter(|v| *v > 0)
        .unwrap_or(reference);

    Ok(MarketSecondSnapshot {
        condition_id: market.cond.clone(),
        asset,
        window,
        elapsed_secs,
        observed_at_ns,
        up,
        down,
        reference_spot_micros: spot,
        window_open_micros: open,
        max_quote_age_ms,
        fee_estimate_micros,
        slippage_estimate_micros,
    })
}

/// Every market whose asset has a configured window strategy (Spec 091).
#[must_use]
pub fn strategy_markets(feed: &WindowsFeed) -> Vec<&WindowsMarket> {
    feed.markets
        .iter()
        .filter(|m| MarketAsset::from_symbol(&m.asset).is_some())
        .collect()
}
