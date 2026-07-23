use common_types::{
    parse_quote_price_micros, parse_reference_quantity_e8, DecimalError, QuotePriceMicros,
    ReferenceQuantityE8,
};
use serde_json::{Map, Value};
use thiserror::Error;

pub const REFERENCE_PAYLOAD_VERSION: u16 = 2;
pub const SOURCE_TIME_UNAVAILABLE_NS: i64 = -1;
const PREFIX_BYTES: usize = 16;

#[derive(Clone, Copy, Debug, Eq, Ord, PartialEq, PartialOrd)]
#[repr(u8)]
pub enum ReferenceSymbol {
    BtcUsdt = 1,
    EthUsdt = 2,
}

impl ReferenceSymbol {
    pub const ALL: [Self; 2] = [Self::BtcUsdt, Self::EthUsdt];

    #[must_use]
    pub const fn as_upper(self) -> &'static str {
        match self {
            Self::BtcUsdt => "BTCUSDT",
            Self::EthUsdt => "ETHUSDT",
        }
    }

    #[must_use]
    pub const fn as_stream(self) -> &'static str {
        match self {
            Self::BtcUsdt => "btcusdt",
            Self::EthUsdt => "ethusdt",
        }
    }

    fn parse(value: &str) -> Result<Self, PayloadError> {
        match value {
            "BTCUSDT" => Ok(Self::BtcUsdt),
            "ETHUSDT" => Ok(Self::EthUsdt),
            _ => Err(PayloadError::UnsupportedSymbol(value.to_owned())),
        }
    }

    fn from_byte(value: u8) -> Result<Self, PayloadError> {
        match value {
            1 => Ok(Self::BtcUsdt),
            2 => Ok(Self::EthUsdt),
            _ => Err(PayloadError::UnknownSymbol(value)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CandleInterval {
    OneHourUtc,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum ReferenceEventKind {
    InProgressCandle = 1,
    FinalizedCandle = 2,
    AggregateTrade = 3,
    BookTicker = 4,
}

impl ReferenceEventKind {
    fn from_byte(value: u8) -> Result<Self, PayloadError> {
        match value {
            1 => Ok(Self::InProgressCandle),
            2 => Ok(Self::FinalizedCandle),
            3 => Ok(Self::AggregateTrade),
            4 => Ok(Self::BookTicker),
            _ => Err(PayloadError::UnknownKind(value)),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CandleData {
    pub symbol: ReferenceSymbol,
    pub interval: CandleInterval,
    pub open_time_ms: i64,
    pub close_time_ms: i64,
    pub first_trade_id: i64,
    pub last_trade_id: i64,
    pub open: QuotePriceMicros,
    pub high: QuotePriceMicros,
    pub low: QuotePriceMicros,
    pub close: QuotePriceMicros,
    pub base_volume: ReferenceQuantityE8,
    pub quote_volume: ReferenceQuantityE8,
    pub trade_count: u64,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InProgressCandle(pub CandleData);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FinalizedCandle(pub CandleData);

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AggregateTrade {
    pub symbol: ReferenceSymbol,
    pub aggregate_trade_id: u64,
    pub price: QuotePriceMicros,
    pub quantity: ReferenceQuantityE8,
    pub first_trade_id: u64,
    pub last_trade_id: u64,
    pub trade_time_ms: i64,
    pub buyer_is_maker: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BookTicker {
    pub symbol: ReferenceSymbol,
    pub update_id: u64,
    pub best_bid: QuotePriceMicros,
    pub best_bid_quantity: ReferenceQuantityE8,
    pub best_ask: QuotePriceMicros,
    pub best_ask_quantity: ReferenceQuantityE8,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferenceEvent {
    InProgressCandle(InProgressCandle),
    FinalizedCandle(FinalizedCandle),
    AggregateTrade(AggregateTrade),
    BookTicker(BookTicker),
}

impl ReferenceEvent {
    #[must_use]
    pub const fn symbol(self) -> ReferenceSymbol {
        match self {
            Self::InProgressCandle(value) => value.0.symbol,
            Self::FinalizedCandle(value) => value.0.symbol,
            Self::AggregateTrade(value) => value.symbol,
            Self::BookTicker(value) => value.symbol,
        }
    }

    #[must_use]
    pub const fn kind(self) -> ReferenceEventKind {
        match self {
            Self::InProgressCandle(_) => ReferenceEventKind::InProgressCandle,
            Self::FinalizedCandle(_) => ReferenceEventKind::FinalizedCandle,
            Self::AggregateTrade(_) => ReferenceEventKind::AggregateTrade,
            Self::BookTicker(_) => ReferenceEventKind::BookTicker,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DecodedReferencePayload {
    pub event_time_ms: Option<i64>,
    pub event: ReferenceEvent,
}

#[derive(Debug, Error)]
pub enum PayloadError {
    #[error("invalid reference JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("combined stream wrapper is malformed")]
    InvalidWrapper,
    #[error("unsupported stream: {0}")]
    UnsupportedStream(String),
    #[error("unsupported event type: {0}")]
    UnsupportedEvent(String),
    #[error("unsupported symbol: {0}")]
    UnsupportedSymbol(String),
    #[error("unknown encoded symbol: {0}")]
    UnknownSymbol(u8),
    #[error("unknown encoded event kind: {0}")]
    UnknownKind(u8),
    #[error("missing or invalid field: {0}")]
    InvalidField(&'static str),
    #[error("invalid decimal field {field}: {source}")]
    Decimal {
        field: &'static str,
        source: DecimalError,
    },
    #[error("only the UTC one-hour candle is accepted")]
    WrongCandleInterval,
    #[error("invalid candle bounds or price relationships")]
    InvalidCandle,
    #[error("invalid or crossed best bid/ask")]
    InvalidBook,
    #[error("payload is truncated")]
    Truncated,
    #[error("payload has trailing bytes")]
    TrailingBytes,
    #[error("unsupported payload version: {0}")]
    UnsupportedVersion(u16),
    #[error("payload reserved byte is set: {0}")]
    ReservedByte(u8),
    #[error("payload prefix and body disagree: {0}")]
    Mismatch(&'static str),
    #[error("timestamp conversion overflow")]
    TimestampOverflow,
}

pub(crate) fn parse_combined_message(text: &str) -> Result<DecodedReferencePayload, PayloadError> {
    let root: Value = serde_json::from_str(text)?;
    let root = root.as_object().ok_or(PayloadError::InvalidWrapper)?;
    let stream = string(root, "stream")?;
    let data = root
        .get("data")
        .and_then(Value::as_object)
        .ok_or(PayloadError::InvalidWrapper)?;
    let event = string(data, "e").ok();
    let decoded = match event {
        Some("kline") => parse_kline(data)?,
        Some("aggTrade") => parse_aggregate_trade(data)?,
        Some(other) => return Err(PayloadError::UnsupportedEvent(other.to_owned())),
        None if stream.ends_with("@bookTicker") => parse_book_ticker(data)?,
        None => return Err(PayloadError::UnsupportedStream(stream.to_owned())),
    };
    validate_stream(stream, decoded.event)?;
    Ok(decoded)
}

fn validate_stream(stream: &str, event: ReferenceEvent) -> Result<(), PayloadError> {
    let symbol = event.symbol().as_stream();
    let expected = match event {
        ReferenceEvent::InProgressCandle(_) | ReferenceEvent::FinalizedCandle(_) => {
            format!("{symbol}@kline_1h")
        }
        ReferenceEvent::AggregateTrade(_) => format!("{symbol}@aggTrade"),
        ReferenceEvent::BookTicker(_) => format!("{symbol}@bookTicker"),
    };
    if stream == expected {
        Ok(())
    } else {
        Err(PayloadError::UnsupportedStream(stream.to_owned()))
    }
}

fn parse_kline(data: &Map<String, Value>) -> Result<DecodedReferencePayload, PayloadError> {
    let event_time_ms = integer_i64(data, "E")?;
    let outer_symbol = ReferenceSymbol::parse(string(data, "s")?)?;
    let k = data
        .get("k")
        .and_then(Value::as_object)
        .ok_or(PayloadError::InvalidField("k"))?;
    let symbol = ReferenceSymbol::parse(string(k, "s")?)?;
    if outer_symbol != symbol {
        return Err(PayloadError::Mismatch("kline symbol"));
    }
    if string(k, "i")? != "1h" {
        return Err(PayloadError::WrongCandleInterval);
    }
    let candle = CandleData {
        symbol,
        interval: CandleInterval::OneHourUtc,
        open_time_ms: integer_i64(k, "t")?,
        close_time_ms: integer_i64(k, "T")?,
        first_trade_id: integer_i64(k, "f")?,
        last_trade_id: integer_i64(k, "L")?,
        open: quote(k, "o")?,
        high: quote(k, "h")?,
        low: quote(k, "l")?,
        close: quote(k, "c")?,
        base_volume: quantity(k, "v")?,
        quote_volume: quantity(k, "q")?,
        trade_count: integer_u64(k, "n")?,
    };
    validate_candle(&candle)?;
    let event = if boolean(k, "x")? {
        ReferenceEvent::FinalizedCandle(FinalizedCandle(candle))
    } else {
        ReferenceEvent::InProgressCandle(InProgressCandle(candle))
    };
    Ok(DecodedReferencePayload {
        event_time_ms: Some(event_time_ms),
        event,
    })
}

fn parse_aggregate_trade(
    data: &Map<String, Value>,
) -> Result<DecodedReferencePayload, PayloadError> {
    let event_time_ms = integer_i64(data, "E")?;
    let trade = AggregateTrade {
        symbol: ReferenceSymbol::parse(string(data, "s")?)?,
        aggregate_trade_id: integer_u64(data, "a")?,
        price: quote(data, "p")?,
        quantity: quantity(data, "q")?,
        first_trade_id: integer_u64(data, "f")?,
        last_trade_id: integer_u64(data, "l")?,
        trade_time_ms: integer_i64(data, "T")?,
        buyer_is_maker: boolean(data, "m")?,
    };
    if trade.first_trade_id > trade.last_trade_id || trade.trade_time_ms < 0 {
        return Err(PayloadError::InvalidField("aggregate trade bounds"));
    }
    Ok(DecodedReferencePayload {
        event_time_ms: Some(event_time_ms),
        event: ReferenceEvent::AggregateTrade(trade),
    })
}

fn parse_book_ticker(data: &Map<String, Value>) -> Result<DecodedReferencePayload, PayloadError> {
    let ticker = BookTicker {
        symbol: ReferenceSymbol::parse(string(data, "s")?)?,
        update_id: integer_u64(data, "u")?,
        best_bid: quote(data, "b")?,
        best_bid_quantity: quantity(data, "B")?,
        best_ask: quote(data, "a")?,
        best_ask_quantity: quantity(data, "A")?,
    };
    if ticker.best_bid > ticker.best_ask {
        return Err(PayloadError::InvalidBook);
    }
    Ok(DecodedReferencePayload {
        event_time_ms: None,
        event: ReferenceEvent::BookTicker(ticker),
    })
}

fn validate_candle(candle: &CandleData) -> Result<(), PayloadError> {
    if candle.open_time_ms < 0
        || candle.close_time_ms <= candle.open_time_ms
        || candle.high < candle.low
        || candle.open > candle.high
        || candle.open < candle.low
        || candle.close > candle.high
        || candle.close < candle.low
    {
        return Err(PayloadError::InvalidCandle);
    }
    Ok(())
}

pub(crate) fn encode_reference_payload(
    decoded: DecodedReferencePayload,
) -> Result<Vec<u8>, PayloadError> {
    let body = encode_body(decoded.event);
    let event_ms = decoded.event_time_ms.unwrap_or(-1);
    let body_len = u32::try_from(body.len()).map_err(|_| PayloadError::Truncated)?;
    let mut output = Vec::with_capacity(PREFIX_BYTES + body.len());
    output.extend_from_slice(&REFERENCE_PAYLOAD_VERSION.to_le_bytes());
    output.push(decoded.event.kind() as u8);
    output.push(decoded.event.symbol() as u8);
    output.extend_from_slice(&event_ms.to_le_bytes());
    output.extend_from_slice(&body_len.to_le_bytes());
    output.extend_from_slice(&body);
    Ok(output)
}

/// Decodes exactly one versioned normalized reference-feed payload.
///
/// # Errors
///
/// Returns [`PayloadError`] for malformed, unsupported, inconsistent, or
/// financially invalid payloads.
pub fn decode_reference_payload(bytes: &[u8]) -> Result<DecodedReferencePayload, PayloadError> {
    if bytes.len() < PREFIX_BYTES {
        return Err(PayloadError::Truncated);
    }
    let version = read_u16(bytes, 0)?;
    if version != REFERENCE_PAYLOAD_VERSION {
        return Err(PayloadError::UnsupportedVersion(version));
    }
    let kind = ReferenceEventKind::from_byte(bytes[2])?;
    let symbol = ReferenceSymbol::from_byte(bytes[3])?;
    let event_ms = read_i64(bytes, 4)?;
    let body_len = usize::try_from(read_u32(bytes, 12)?).map_err(|_| PayloadError::Truncated)?;
    let end = PREFIX_BYTES
        .checked_add(body_len)
        .ok_or(PayloadError::Truncated)?;
    let body = bytes
        .get(PREFIX_BYTES..end)
        .ok_or(PayloadError::Truncated)?;
    if end != bytes.len() {
        return Err(PayloadError::TrailingBytes);
    }
    let event = decode_body(kind, symbol, body)?;
    let event_time_ms = if event_ms == -1 {
        None
    } else if event_ms >= 0 {
        Some(event_ms)
    } else {
        return Err(PayloadError::InvalidField("event time"));
    };
    if matches!(event, ReferenceEvent::BookTicker(_)) != event_time_ms.is_none() {
        return Err(PayloadError::Mismatch("source event-time availability"));
    }
    Ok(DecodedReferencePayload {
        event_time_ms,
        event,
    })
}

fn encode_body(event: ReferenceEvent) -> Vec<u8> {
    let mut out = Vec::new();
    match event {
        ReferenceEvent::InProgressCandle(value) => encode_candle(&mut out, value.0),
        ReferenceEvent::FinalizedCandle(value) => encode_candle(&mut out, value.0),
        ReferenceEvent::AggregateTrade(value) => {
            push_u64(&mut out, value.aggregate_trade_id);
            push_i64(&mut out, value.price.as_micros());
            push_i64(&mut out, value.quantity.as_e8());
            push_u64(&mut out, value.first_trade_id);
            push_u64(&mut out, value.last_trade_id);
            push_i64(&mut out, value.trade_time_ms);
            out.push(u8::from(value.buyer_is_maker));
        }
        ReferenceEvent::BookTicker(value) => {
            push_u64(&mut out, value.update_id);
            push_i64(&mut out, value.best_bid.as_micros());
            push_i64(&mut out, value.best_bid_quantity.as_e8());
            push_i64(&mut out, value.best_ask.as_micros());
            push_i64(&mut out, value.best_ask_quantity.as_e8());
        }
    }
    out
}

fn encode_candle(out: &mut Vec<u8>, value: CandleData) {
    push_i64(out, value.open_time_ms);
    push_i64(out, value.close_time_ms);
    push_i64(out, value.first_trade_id);
    push_i64(out, value.last_trade_id);
    push_i64(out, value.open.as_micros());
    push_i64(out, value.high.as_micros());
    push_i64(out, value.low.as_micros());
    push_i64(out, value.close.as_micros());
    push_i64(out, value.base_volume.as_e8());
    push_i64(out, value.quote_volume.as_e8());
    push_u64(out, value.trade_count);
}

fn decode_body(
    kind: ReferenceEventKind,
    symbol: ReferenceSymbol,
    body: &[u8],
) -> Result<ReferenceEvent, PayloadError> {
    match kind {
        ReferenceEventKind::InProgressCandle | ReferenceEventKind::FinalizedCandle => {
            if body.len() != 88 {
                return Err(PayloadError::Truncated);
            }
            let candle = CandleData {
                symbol,
                interval: CandleInterval::OneHourUtc,
                open_time_ms: read_i64(body, 0)?,
                close_time_ms: read_i64(body, 8)?,
                first_trade_id: read_i64(body, 16)?,
                last_trade_id: read_i64(body, 24)?,
                open: quote_from_i64(read_i64(body, 32)?)?,
                high: quote_from_i64(read_i64(body, 40)?)?,
                low: quote_from_i64(read_i64(body, 48)?)?,
                close: quote_from_i64(read_i64(body, 56)?)?,
                base_volume: quantity_from_i64(read_i64(body, 64)?)?,
                quote_volume: quantity_from_i64(read_i64(body, 72)?)?,
                trade_count: read_u64(body, 80)?,
            };
            validate_candle(&candle)?;
            Ok(if kind == ReferenceEventKind::FinalizedCandle {
                ReferenceEvent::FinalizedCandle(FinalizedCandle(candle))
            } else {
                ReferenceEvent::InProgressCandle(InProgressCandle(candle))
            })
        }
        ReferenceEventKind::AggregateTrade => {
            if body.len() != 49 {
                return Err(PayloadError::Truncated);
            }
            let event = AggregateTrade {
                symbol,
                aggregate_trade_id: read_u64(body, 0)?,
                price: quote_from_i64(read_i64(body, 8)?)?,
                quantity: quantity_from_i64(read_i64(body, 16)?)?,
                first_trade_id: read_u64(body, 24)?,
                last_trade_id: read_u64(body, 32)?,
                trade_time_ms: read_i64(body, 40)?,
                buyer_is_maker: match body[48] {
                    0 => false,
                    1 => true,
                    _ => return Err(PayloadError::InvalidField("buyer maker")),
                },
            };
            Ok(ReferenceEvent::AggregateTrade(event))
        }
        ReferenceEventKind::BookTicker => {
            if body.len() != 40 {
                return Err(PayloadError::Truncated);
            }
            let event = BookTicker {
                symbol,
                update_id: read_u64(body, 0)?,
                best_bid: quote_from_i64(read_i64(body, 8)?)?,
                best_bid_quantity: quantity_from_i64(read_i64(body, 16)?)?,
                best_ask: quote_from_i64(read_i64(body, 24)?)?,
                best_ask_quantity: quantity_from_i64(read_i64(body, 32)?)?,
            };
            if event.best_bid > event.best_ask {
                return Err(PayloadError::InvalidBook);
            }
            Ok(ReferenceEvent::BookTicker(event))
        }
    }
}

fn string<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<&'a str, PayloadError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|v| !v.is_empty())
        .ok_or(PayloadError::InvalidField(field))
}
fn integer_i64(object: &Map<String, Value>, field: &'static str) -> Result<i64, PayloadError> {
    object
        .get(field)
        .and_then(Value::as_i64)
        .filter(|v| *v >= 0)
        .ok_or(PayloadError::InvalidField(field))
}
fn integer_u64(object: &Map<String, Value>, field: &'static str) -> Result<u64, PayloadError> {
    object
        .get(field)
        .and_then(Value::as_u64)
        .ok_or(PayloadError::InvalidField(field))
}
fn boolean(object: &Map<String, Value>, field: &'static str) -> Result<bool, PayloadError> {
    object
        .get(field)
        .and_then(Value::as_bool)
        .ok_or(PayloadError::InvalidField(field))
}
fn quote(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<QuotePriceMicros, PayloadError> {
    parse_quote_price_micros(string(object, field)?)
        .map_err(|source| PayloadError::Decimal { field, source })
}
fn quantity(
    object: &Map<String, Value>,
    field: &'static str,
) -> Result<ReferenceQuantityE8, PayloadError> {
    parse_reference_quantity_e8(string(object, field)?)
        .map_err(|source| PayloadError::Decimal { field, source })
}
fn quote_from_i64(value: i64) -> Result<QuotePriceMicros, PayloadError> {
    QuotePriceMicros::new(value).map_err(|e| PayloadError::Decimal {
        field: "encoded quote",
        source: DecimalError::Financial(e),
    })
}
fn quantity_from_i64(value: i64) -> Result<ReferenceQuantityE8, PayloadError> {
    ReferenceQuantityE8::new(value).map_err(|e| PayloadError::Decimal {
        field: "encoded quantity",
        source: DecimalError::Financial(e),
    })
}
fn push_i64(out: &mut Vec<u8>, value: i64) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn push_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_le_bytes());
}
fn read_u16(bytes: &[u8], offset: usize) -> Result<u16, PayloadError> {
    Ok(u16::from_le_bytes(
        bytes
            .get(offset..offset + 2)
            .ok_or(PayloadError::Truncated)?
            .try_into()
            .map_err(|_| PayloadError::Truncated)?,
    ))
}
fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, PayloadError> {
    Ok(u32::from_le_bytes(
        bytes
            .get(offset..offset + 4)
            .ok_or(PayloadError::Truncated)?
            .try_into()
            .map_err(|_| PayloadError::Truncated)?,
    ))
}
fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, PayloadError> {
    Ok(u64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(PayloadError::Truncated)?
            .try_into()
            .map_err(|_| PayloadError::Truncated)?,
    ))
}
fn read_i64(bytes: &[u8], offset: usize) -> Result<i64, PayloadError> {
    Ok(i64::from_le_bytes(
        bytes
            .get(offset..offset + 8)
            .ok_or(PayloadError::Truncated)?
            .try_into()
            .map_err(|_| PayloadError::Truncated)?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_and_round_trips_all_public_stream_types() {
        let samples = [
            r#"{"stream":"btcusdt@aggTrade","data":{"e":"aggTrade","E":1498793709153,"s":"BTCUSDT","a":5933014,"p":"0.00100000","q":"0.12345678","f":100,"l":105,"T":1498793709153,"m":true,"M":true}}"#,
            r#"{"stream":"ethusdt@bookTicker","data":{"u":400900217,"s":"ETHUSDT","b":"25.35190000","B":"31.21000000","a":"25.36520000","A":"40.66000000"}}"#,
            r#"{"stream":"btcusdt@kline_1h","data":{"e":"kline","E":1672515782136,"s":"BTCUSDT","k":{"t":1672515780000,"T":1672519379999,"s":"BTCUSDT","i":"1h","f":100,"L":200,"o":"100.0","c":"101.0","h":"102.0","l":"99.0","v":"1000.0","n":100,"x":true,"q":"100000.0"}}}"#,
        ];
        for sample in samples {
            let parsed = parse_combined_message(sample).expect("parse");
            let encoded = encode_reference_payload(parsed).expect("encode");
            assert_eq!(decode_reference_payload(&encoded).expect("decode"), parsed);
        }
    }

    #[test]
    fn book_ticker_never_invents_source_time() {
        let sample = r#"{"stream":"btcusdt@bookTicker","data":{"u":1,"s":"BTCUSDT","b":"100","B":"1","a":"101","A":"2"}}"#;
        let parsed = parse_combined_message(sample).expect("parse");
        assert_eq!(parsed.event_time_ms, None);
        assert_eq!(
            decode_reference_payload(&encode_reference_payload(parsed).expect("encode"))
                .expect("decode")
                .event_time_ms,
            None
        );
    }

    #[test]
    fn rejects_wrong_interval_crossed_book_and_precision_loss() {
        let wrong = r#"{"stream":"btcusdt@kline_1h","data":{"e":"kline","E":1,"s":"BTCUSDT","k":{"t":1,"T":2,"s":"BTCUSDT","i":"1m","f":1,"L":1,"o":"1","c":"1","h":"1","l":"1","v":"1","n":1,"x":false,"q":"1"}}}"#;
        assert!(matches!(
            parse_combined_message(wrong),
            Err(PayloadError::WrongCandleInterval)
        ));
        let crossed = r#"{"stream":"btcusdt@bookTicker","data":{"u":1,"s":"BTCUSDT","b":"102","B":"1","a":"101","A":"2"}}"#;
        assert!(matches!(
            parse_combined_message(crossed),
            Err(PayloadError::InvalidBook)
        ));
        let precision = r#"{"stream":"btcusdt@bookTicker","data":{"u":1,"s":"BTCUSDT","b":"100.0000001","B":"1","a":"101","A":"2"}}"#;
        assert!(matches!(
            parse_combined_message(precision),
            Err(PayloadError::Decimal { .. })
        ));
    }
}
