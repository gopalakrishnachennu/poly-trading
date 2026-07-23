//! Strict decoding for the versioned public-market journal payload.

use crate::domain::{validate_hex_id, validate_token_id, IdentityError};
use common_types::{
    parse_price_micros, parse_quantity_micros, DecimalError, PriceMicros, QuantityMicros,
};
use serde::Deserialize;
use serde_json::Value;
use thiserror::Error;

pub(crate) const PUBLIC_EVENT_PAYLOAD_VERSION: u16 = 1;
const FIXED_PREFIX_BYTES: usize = 18;
const MAX_ASSETS: usize = 1_024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum PublicEventKind {
    Book = 1,
    PriceChange = 2,
    TickSizeChange = 3,
    LastTradePrice = 4,
    BestBidAsk = 5,
}

impl PublicEventKind {
    pub(crate) fn parse(value: &str) -> Result<Self, PayloadError> {
        match value {
            "book" => Ok(Self::Book),
            "price_change" => Ok(Self::PriceChange),
            "tick_size_change" => Ok(Self::TickSizeChange),
            "last_trade_price" => Ok(Self::LastTradePrice),
            "best_bid_ask" => Ok(Self::BestBidAsk),
            _ => Err(PayloadError::UnknownEventType(value.to_owned())),
        }
    }

    fn from_byte(value: u8) -> Result<Self, PayloadError> {
        match value {
            1 => Ok(Self::Book),
            2 => Ok(Self::PriceChange),
            3 => Ok(Self::TickSizeChange),
            4 => Ok(Self::LastTradePrice),
            5 => Ok(Self::BestBidAsk),
            _ => Err(PayloadError::UnknownKind(value)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MarketSide {
    Bid,
    Ask,
}

impl MarketSide {
    fn parse(value: &str) -> Result<Self, PayloadError> {
        match value {
            "BUY" => Ok(Self::Bid),
            "SELL" => Ok(Self::Ask),
            _ => Err(PayloadError::InvalidSide(value.to_owned())),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BookLevel {
    pub price: PriceMicros,
    pub quantity: QuantityMicros,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BookSnapshot {
    pub condition_id: String,
    pub asset_id: String,
    pub bids: Vec<BookLevel>,
    pub asks: Vec<BookLevel>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PriceChange {
    pub asset_id: String,
    pub side: MarketSide,
    pub price: PriceMicros,
    pub quantity: QuantityMicros,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TickSizeChange {
    pub condition_id: String,
    pub asset_id: String,
    pub old_tick_size: PriceMicros,
    pub new_tick_size: PriceMicros,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LastTrade {
    pub condition_id: String,
    pub asset_id: String,
    pub side: MarketSide,
    pub price: PriceMicros,
    pub quantity: Option<QuantityMicros>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct BestBidAsk {
    pub condition_id: String,
    pub asset_id: String,
    pub best_bid: PriceMicros,
    pub best_ask: PriceMicros,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum PublicMarketEvent {
    Book(BookSnapshot),
    PriceChanges {
        condition_id: String,
        changes: Vec<PriceChange>,
    },
    TickSizeChange(TickSizeChange),
    LastTrade(LastTrade),
    BestBidAsk(BestBidAsk),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DecodedPublicPayload {
    pub kind: PublicEventKind,
    pub timestamp_ms: i64,
    pub asset_ids: Vec<String>,
    pub event: PublicMarketEvent,
}

#[derive(Debug, Error)]
pub enum PayloadError {
    #[error("public payload is truncated")]
    Truncated,
    #[error("unsupported public payload version: {0}")]
    UnsupportedVersion(u16),
    #[error("public payload reserved byte is set: {0}")]
    ReservedByte(u8),
    #[error("unknown public event kind: {0}")]
    UnknownKind(u8),
    #[error("unknown public JSON event type: {0}")]
    UnknownEventType(String),
    #[error("public payload has zero or too many assets")]
    InvalidAssetCount,
    #[error("public payload has trailing bytes")]
    TrailingBytes,
    #[error("public payload string is not UTF-8")]
    InvalidUtf8,
    #[error("invalid public payload JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("invalid public identity: {0}")]
    Identity(#[from] IdentityError),
    #[error("invalid decimal field {field}: {source}")]
    Decimal {
        field: &'static str,
        source: DecimalError,
    },
    #[error("invalid market side: {0}")]
    InvalidSide(String),
    #[error("payload prefix and JSON disagree: {0}")]
    Mismatch(&'static str),
    #[error("event contains no price changes")]
    EmptyPriceChanges,
}

/// Decodes exactly one version-one normalized public payload.
///
/// # Errors
///
/// Returns [`PayloadError`] on structural, identity, fixed-point, or
/// prefix-to-JSON disagreement.
pub fn decode_public_payload(bytes: &[u8]) -> Result<DecodedPublicPayload, PayloadError> {
    if bytes.len() < FIXED_PREFIX_BYTES {
        return Err(PayloadError::Truncated);
    }
    let version = read_u16(bytes, 0)?;
    if version != PUBLIC_EVENT_PAYLOAD_VERSION {
        return Err(PayloadError::UnsupportedVersion(version));
    }
    let kind = PublicEventKind::from_byte(bytes[2])?;
    if bytes[3] != 0 {
        return Err(PayloadError::ReservedByte(bytes[3]));
    }
    let timestamp_ms = read_i64(bytes, 4)?;
    if timestamp_ms < 0 {
        return Err(PayloadError::Mismatch("negative timestamp"));
    }
    let asset_count = usize::from(read_u16(bytes, 12)?);
    if asset_count == 0 || asset_count > MAX_ASSETS {
        return Err(PayloadError::InvalidAssetCount);
    }
    let mut offset = 14_usize;
    let mut asset_ids = Vec::with_capacity(asset_count);
    for _ in 0..asset_count {
        let length = usize::from(read_u16(bytes, offset)?);
        offset = offset.checked_add(2).ok_or(PayloadError::Truncated)?;
        let end = offset.checked_add(length).ok_or(PayloadError::Truncated)?;
        let asset = std::str::from_utf8(bytes.get(offset..end).ok_or(PayloadError::Truncated)?)
            .map_err(|_| PayloadError::InvalidUtf8)?
            .to_owned();
        validate_token_id(&asset)?;
        asset_ids.push(asset);
        offset = end;
    }
    let json_length =
        usize::try_from(read_u32(bytes, offset)?).map_err(|_| PayloadError::Truncated)?;
    offset = offset.checked_add(4).ok_or(PayloadError::Truncated)?;
    let end = offset
        .checked_add(json_length)
        .ok_or(PayloadError::Truncated)?;
    let json = bytes.get(offset..end).ok_or(PayloadError::Truncated)?;
    if end != bytes.len() {
        return Err(PayloadError::TrailingBytes);
    }

    let value: Value = serde_json::from_slice(json)?;
    let raw_header: RawHeader = serde_json::from_value(value.clone())?;
    let json_kind = PublicEventKind::parse(&raw_header.event_type)?;
    if json_kind != kind {
        return Err(PayloadError::Mismatch("event kind"));
    }
    if parse_timestamp(&raw_header.timestamp)? != timestamp_ms {
        return Err(PayloadError::Mismatch("timestamp"));
    }
    validate_hex_id("market", &raw_header.market)?;

    let event = decode_event(kind, value, &raw_header.market)?;
    let json_assets = event_asset_ids(&event);
    if json_assets != asset_ids {
        return Err(PayloadError::Mismatch("asset IDs"));
    }
    Ok(DecodedPublicPayload {
        kind,
        timestamp_ms,
        asset_ids,
        event,
    })
}

fn decode_event(
    kind: PublicEventKind,
    value: Value,
    condition_id: &str,
) -> Result<PublicMarketEvent, PayloadError> {
    match kind {
        PublicEventKind::Book => {
            let raw: RawBook = serde_json::from_value(value)?;
            validate_token_id(&raw.asset_id)?;
            Ok(PublicMarketEvent::Book(BookSnapshot {
                condition_id: condition_id.to_owned(),
                asset_id: raw.asset_id,
                bids: parse_levels(raw.bids)?,
                asks: parse_levels(raw.asks)?,
            }))
        }
        PublicEventKind::PriceChange => {
            let raw: RawPriceChanges = serde_json::from_value(value)?;
            if raw.price_changes.is_empty() {
                return Err(PayloadError::EmptyPriceChanges);
            }
            let changes = raw
                .price_changes
                .into_iter()
                .map(|change| {
                    validate_token_id(&change.asset_id)?;
                    Ok(PriceChange {
                        asset_id: change.asset_id,
                        side: MarketSide::parse(&change.side)?,
                        price: price("price", &change.price)?,
                        quantity: quantity("size", &change.size)?,
                    })
                })
                .collect::<Result<_, PayloadError>>()?;
            Ok(PublicMarketEvent::PriceChanges {
                condition_id: condition_id.to_owned(),
                changes,
            })
        }
        PublicEventKind::TickSizeChange => {
            let raw: RawTickSize = serde_json::from_value(value)?;
            validate_token_id(&raw.asset_id)?;
            Ok(PublicMarketEvent::TickSizeChange(TickSizeChange {
                condition_id: condition_id.to_owned(),
                asset_id: raw.asset_id,
                old_tick_size: price("old_tick_size", &raw.old_tick_size)?,
                new_tick_size: price("new_tick_size", &raw.new_tick_size)?,
            }))
        }
        PublicEventKind::LastTradePrice => {
            let raw: RawLastTrade = serde_json::from_value(value)?;
            validate_token_id(&raw.asset_id)?;
            Ok(PublicMarketEvent::LastTrade(LastTrade {
                condition_id: condition_id.to_owned(),
                asset_id: raw.asset_id,
                side: MarketSide::parse(&raw.side)?,
                price: price("price", &raw.price)?,
                quantity: raw
                    .size
                    .as_deref()
                    .map(|value| quantity("size", value))
                    .transpose()?,
            }))
        }
        PublicEventKind::BestBidAsk => {
            let raw: RawBestBidAsk = serde_json::from_value(value)?;
            validate_token_id(&raw.asset_id)?;
            Ok(PublicMarketEvent::BestBidAsk(BestBidAsk {
                condition_id: condition_id.to_owned(),
                asset_id: raw.asset_id,
                best_bid: price("best_bid", &raw.best_bid)?,
                best_ask: price("best_ask", &raw.best_ask)?,
            }))
        }
    }
}

fn event_asset_ids(event: &PublicMarketEvent) -> Vec<String> {
    match event {
        PublicMarketEvent::Book(event) => vec![event.asset_id.clone()],
        PublicMarketEvent::PriceChanges { changes, .. } => changes
            .iter()
            .map(|change| change.asset_id.clone())
            .collect(),
        PublicMarketEvent::TickSizeChange(event) => vec![event.asset_id.clone()],
        PublicMarketEvent::LastTrade(event) => vec![event.asset_id.clone()],
        PublicMarketEvent::BestBidAsk(event) => vec![event.asset_id.clone()],
    }
}

fn parse_levels(levels: Vec<RawLevel>) -> Result<Vec<BookLevel>, PayloadError> {
    levels
        .into_iter()
        .map(|level| {
            Ok(BookLevel {
                price: price("price", &level.price)?,
                quantity: quantity("size", &level.size)?,
            })
        })
        .collect()
}

fn price(field: &'static str, value: &str) -> Result<PriceMicros, PayloadError> {
    parse_price_micros(value).map_err(|source| PayloadError::Decimal { field, source })
}

fn quantity(field: &'static str, value: &str) -> Result<QuantityMicros, PayloadError> {
    parse_quantity_micros(value).map_err(|source| PayloadError::Decimal { field, source })
}

fn parse_timestamp(value: &Value) -> Result<i64, PayloadError> {
    let timestamp = match value {
        Value::String(value) => value
            .parse()
            .map_err(|_| PayloadError::Mismatch("timestamp"))?,
        Value::Number(value) => value.as_i64().ok_or(PayloadError::Mismatch("timestamp"))?,
        _ => return Err(PayloadError::Mismatch("timestamp")),
    };
    if timestamp < 0 {
        return Err(PayloadError::Mismatch("negative timestamp"));
    }
    Ok(timestamp)
}

fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, PayloadError> {
    let value = bytes
        .get(offset..offset + 2)
        .ok_or(PayloadError::Truncated)?;
    Ok(u16::from_le_bytes([value[0], value[1]]))
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, PayloadError> {
    let value = bytes
        .get(offset..offset + 4)
        .ok_or(PayloadError::Truncated)?;
    Ok(u32::from_le_bytes(
        value.try_into().map_err(|_| PayloadError::Truncated)?,
    ))
}

fn read_i64(bytes: &[u8], offset: usize) -> Result<i64, PayloadError> {
    let value = bytes
        .get(offset..offset + 8)
        .ok_or(PayloadError::Truncated)?;
    Ok(i64::from_le_bytes(
        value.try_into().map_err(|_| PayloadError::Truncated)?,
    ))
}

#[derive(Clone, Debug, Deserialize)]
struct RawHeader {
    event_type: String,
    market: String,
    timestamp: Value,
}

#[derive(Clone, Debug, Deserialize)]
struct RawLevel {
    price: String,
    size: String,
}

#[derive(Debug, Deserialize)]
struct RawBook {
    asset_id: String,
    #[serde(default)]
    bids: Vec<RawLevel>,
    #[serde(default)]
    asks: Vec<RawLevel>,
}

#[derive(Debug, Deserialize)]
struct RawPriceChanges {
    price_changes: Vec<RawPriceChange>,
}

#[derive(Debug, Deserialize)]
struct RawPriceChange {
    asset_id: String,
    side: String,
    price: String,
    size: String,
}

#[derive(Debug, Deserialize)]
struct RawTickSize {
    asset_id: String,
    old_tick_size: String,
    new_tick_size: String,
}

#[derive(Debug, Deserialize)]
struct RawLastTrade {
    asset_id: String,
    side: String,
    price: String,
    #[serde(default)]
    size: Option<String>,
}

#[derive(Debug, Deserialize)]
struct RawBestBidAsk {
    asset_id: String,
    best_bid: String,
    best_ask: String,
}
