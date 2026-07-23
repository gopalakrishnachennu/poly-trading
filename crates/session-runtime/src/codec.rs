use common_types::{PriceMicros, QuantityMicros, QuotePriceMicros, ReferenceQuantityE8};
use feed_supervisor::{SupervisorMode, SupervisorSnapshot};
use live_market_state::{ActorMode, ActorSnapshot};
use market_session::{CoordinationFrame, SessionKey, SessionSourceState, TokenBookView};
use public_market_data::{Asset, MarketIdentity};
use reference_market_data::{
    CandleData, CandleInterval, FinalizedCandle, InProgressCandle, ReferenceHealth,
    ReferenceSnapshot, ReferenceSymbol, ReferenceSymbolSnapshot,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use thiserror::Error;

const WIRE_VERSION: u16 = 2;
const MAX_SESSIONS: usize = 4_096;
const MAX_TEXT_BYTES: usize = 128 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum DurableCommand {
    Register {
        identity: MarketIdentity,
        recorded_at_ns: i64,
    },
    Coordinate(CoordinationFrame),
}

impl DurableCommand {
    #[must_use]
    pub const fn timestamp_ns(&self) -> i64 {
        match self {
            Self::Register { recorded_at_ns, .. } => *recorded_at_ns,
            Self::Coordinate(frame) => frame.now_ns,
        }
    }
}

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum CodecError {
    #[error("session command JSON is invalid: {0}")]
    Json(String),
    #[error("unsupported session command version: {0}")]
    Version(u16),
    #[error("session command exceeds a collection or string bound")]
    Bound,
    #[error("session command contains an invalid enum value")]
    Enum,
    #[error("session command contains invalid fixed-point data")]
    Financial,
    #[error("session command contains a duplicate or inconsistent key")]
    Duplicate,
    #[error("session command timestamp is invalid")]
    Timestamp,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCommand {
    version: u16,
    command: WireCommandKind,
}

#[derive(Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
enum WireCommandKind {
    Register {
        identity: WireIdentity,
        recorded_at_ns: i64,
    },
    Coordinate(WireFrame),
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireIdentity {
    asset: u8,
    event_id: String,
    market_id: String,
    condition_id: String,
    question_id: String,
    event_slug: String,
    market_slug: String,
    series_id: String,
    series_slug: String,
    title: String,
    start_time_ms: i64,
    end_time_ms: i64,
    resolution_source: String,
    description: String,
    up_token_id: String,
    down_token_id: String,
    rules_fingerprint: [u8; 32],
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireFrame {
    now_ns: i64,
    market: WireActor,
    reference: WireReference,
    supervision: WireSupervisor,
    sessions: Vec<WireSession>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireActor {
    mode: u8,
    ready: bool,
    epoch: u64,
    last_sequence: Option<u64>,
    book_count: usize,
    digest: [u8; 32],
    last_market_event_ns: Option<i64>,
    last_market_received_ns: Option<i64>,
    halt_reason: Option<String>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireReference {
    health: u8,
    epoch: u64,
    last_sequence: Option<u64>,
    digest: [u8; 32],
    last_reference_received_ns: Option<i64>,
    symbols: Vec<WireSymbolTiming>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireSymbolTiming {
    symbol: u8,
    candle_event_ns: Option<i64>,
    candle_received_ns: Option<i64>,
    aggregate_trade_event_ns: Option<i64>,
    aggregate_trade_received_ns: Option<i64>,
    book_ticker_received_ns: Option<i64>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireSupervisor {
    mode: u8,
    ready: bool,
    evaluated_at_ns: Option<i64>,
    market_epoch: u64,
    market_sequence: Option<u64>,
    market_digest: [u8; 32],
    market_state_digest: [u8; 32],
    reference_epoch: u64,
    reference_sequence: Option<u64>,
    reference_digest: [u8; 32],
    reference_state_digest: [u8; 32],
    halt_reason: Option<String>,
    digest: [u8; 32],
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireSession {
    asset: u8,
    start_time_ms: i64,
    up_book: Option<WireBook>,
    down_book: Option<WireBook>,
    in_progress: Option<WireCandle>,
    finalized: Option<WireCandle>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireBook {
    authoritative: bool,
    best_bid: Option<WireLevel>,
    best_ask: Option<WireLevel>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireLevel {
    price_micros: i64,
    quantity_micros: i64,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct WireCandle {
    symbol: u8,
    open_time_ms: i64,
    close_time_ms: i64,
    first_trade_id: i64,
    last_trade_id: i64,
    open_micros: i64,
    high_micros: i64,
    low_micros: i64,
    close_micros: i64,
    base_volume_e8: i64,
    quote_volume_e8: i64,
    trade_count: u64,
}

/// Encodes one bounded canonical durable command.
///
/// # Errors
///
/// Rejects invalid timestamps, bounds, or JSON serialization failures.
pub fn encode(command: &DurableCommand) -> Result<Vec<u8>, CodecError> {
    validate_command(command)?;
    let command = match command {
        DurableCommand::Register {
            identity,
            recorded_at_ns,
        } => WireCommandKind::Register {
            identity: WireIdentity::from(identity),
            recorded_at_ns: *recorded_at_ns,
        },
        DurableCommand::Coordinate(frame) => WireCommandKind::Coordinate(WireFrame::from(frame)),
    };
    serde_json::to_vec(&WireCommand {
        version: WIRE_VERSION,
        command,
    })
    .map_err(|error| CodecError::Json(error.to_string()))
}

/// Decodes and validates one exact durable command without trailing data.
///
/// # Errors
///
/// Rejects malformed JSON, versions, enums, duplicates, bounds, timestamps,
/// and invalid fixed-point values.
pub fn decode(bytes: &[u8]) -> Result<DurableCommand, CodecError> {
    let wire: WireCommand =
        serde_json::from_slice(bytes).map_err(|error| CodecError::Json(error.to_string()))?;
    if wire.version != WIRE_VERSION {
        return Err(CodecError::Version(wire.version));
    }
    let command = match wire.command {
        WireCommandKind::Register {
            identity,
            recorded_at_ns,
        } => DurableCommand::Register {
            identity: identity.try_into()?,
            recorded_at_ns,
        },
        WireCommandKind::Coordinate(frame) => DurableCommand::Coordinate(frame.try_into()?),
    };
    validate_command(&command)?;
    Ok(command)
}

fn validate_command(command: &DurableCommand) -> Result<(), CodecError> {
    if command.timestamp_ns() < 0 {
        return Err(CodecError::Timestamp);
    }
    match command {
        DurableCommand::Register { identity, .. } => validate_identity(identity),
        DurableCommand::Coordinate(frame) => {
            if frame.sessions.len() > MAX_SESSIONS {
                return Err(CodecError::Bound);
            }
            validate_optional_text(frame.market.halt_reason.as_deref())?;
            validate_optional_text(frame.supervision.halt_reason.as_deref())?;
            Ok(())
        }
    }
}

fn validate_identity(identity: &MarketIdentity) -> Result<(), CodecError> {
    for value in [
        &identity.event_id,
        &identity.market_id,
        &identity.condition_id,
        &identity.question_id,
        &identity.event_slug,
        &identity.market_slug,
        &identity.series_id,
        &identity.series_slug,
        &identity.title,
        &identity.resolution_source,
        &identity.description,
        &identity.up_token_id,
        &identity.down_token_id,
    ] {
        if value.is_empty() || value.len() > MAX_TEXT_BYTES {
            return Err(CodecError::Bound);
        }
    }
    Ok(())
}

fn validate_optional_text(value: Option<&str>) -> Result<(), CodecError> {
    if value.is_some_and(|text| text.len() > MAX_TEXT_BYTES) {
        Err(CodecError::Bound)
    } else {
        Ok(())
    }
}

impl From<&MarketIdentity> for WireIdentity {
    fn from(value: &MarketIdentity) -> Self {
        Self {
            asset: value.asset as u8,
            event_id: value.event_id.clone(),
            market_id: value.market_id.clone(),
            condition_id: value.condition_id.clone(),
            question_id: value.question_id.clone(),
            event_slug: value.event_slug.clone(),
            market_slug: value.market_slug.clone(),
            series_id: value.series_id.clone(),
            series_slug: value.series_slug.clone(),
            title: value.title.clone(),
            start_time_ms: value.start_time_ms,
            end_time_ms: value.end_time_ms,
            resolution_source: value.resolution_source.clone(),
            description: value.description.clone(),
            up_token_id: value.up_token_id.clone(),
            down_token_id: value.down_token_id.clone(),
            rules_fingerprint: value.rules_fingerprint,
        }
    }
}

impl TryFrom<WireIdentity> for MarketIdentity {
    type Error = CodecError;

    fn try_from(value: WireIdentity) -> Result<Self, Self::Error> {
        Ok(Self {
            asset: asset(value.asset)?,
            event_id: value.event_id,
            market_id: value.market_id,
            condition_id: value.condition_id,
            question_id: value.question_id,
            event_slug: value.event_slug,
            market_slug: value.market_slug,
            series_id: value.series_id,
            series_slug: value.series_slug,
            title: value.title,
            start_time_ms: value.start_time_ms,
            end_time_ms: value.end_time_ms,
            resolution_source: value.resolution_source,
            description: value.description,
            up_token_id: value.up_token_id,
            down_token_id: value.down_token_id,
            rules_fingerprint: value.rules_fingerprint,
        })
    }
}

impl From<&CoordinationFrame> for WireFrame {
    fn from(value: &CoordinationFrame) -> Self {
        Self {
            now_ns: value.now_ns,
            market: WireActor::from(&value.market),
            reference: WireReference::from(&value.reference),
            supervision: WireSupervisor::from(&value.supervision),
            sessions: value
                .sessions
                .iter()
                .map(|(key, source)| WireSession::from((*key, source)))
                .collect(),
        }
    }
}

impl TryFrom<WireFrame> for CoordinationFrame {
    type Error = CodecError;

    fn try_from(value: WireFrame) -> Result<Self, Self::Error> {
        if value.sessions.len() > MAX_SESSIONS {
            return Err(CodecError::Bound);
        }
        let mut sessions = BTreeMap::new();
        for session in value.sessions {
            let (key, source) = session.try_into()?;
            if sessions.insert(key, source).is_some() {
                return Err(CodecError::Duplicate);
            }
        }
        Ok(Self {
            now_ns: value.now_ns,
            market: value.market.try_into()?,
            reference: value.reference.try_into()?,
            supervision: value.supervision.try_into()?,
            sessions,
        })
    }
}

impl From<&ActorSnapshot> for WireActor {
    fn from(value: &ActorSnapshot) -> Self {
        Self {
            mode: actor_mode_byte(value.mode),
            ready: value.ready,
            epoch: value.epoch,
            last_sequence: value.last_sequence,
            book_count: value.book_count,
            digest: value.digest,
            last_market_event_ns: value.last_market_event_ns,
            last_market_received_ns: value.last_market_received_ns,
            halt_reason: value.halt_reason.clone(),
        }
    }
}

impl TryFrom<WireActor> for ActorSnapshot {
    type Error = CodecError;

    fn try_from(value: WireActor) -> Result<Self, Self::Error> {
        Ok(Self {
            mode: actor_mode(value.mode)?,
            ready: value.ready,
            epoch: value.epoch,
            last_sequence: value.last_sequence,
            book_count: value.book_count,
            digest: value.digest,
            last_market_event_ns: value.last_market_event_ns,
            last_market_received_ns: value.last_market_received_ns,
            halt_reason: value.halt_reason,
        })
    }
}

impl From<&ReferenceSnapshot> for WireReference {
    fn from(value: &ReferenceSnapshot) -> Self {
        Self {
            health: reference_health_byte(value.health),
            epoch: value.epoch,
            last_sequence: value.last_sequence,
            digest: value.digest,
            last_reference_received_ns: value.last_reference_received_ns,
            symbols: value
                .symbols
                .iter()
                .map(|(symbol, timing)| WireSymbolTiming::from((*symbol, *timing)))
                .collect(),
        }
    }
}

impl TryFrom<WireReference> for ReferenceSnapshot {
    type Error = CodecError;

    fn try_from(value: WireReference) -> Result<Self, Self::Error> {
        let mut symbols = BTreeMap::new();
        for timing in value.symbols {
            let (symbol, timing) = timing.try_into()?;
            if symbols.insert(symbol, timing).is_some() {
                return Err(CodecError::Duplicate);
            }
        }
        Ok(Self {
            health: reference_health(value.health)?,
            epoch: value.epoch,
            last_sequence: value.last_sequence,
            digest: value.digest,
            last_reference_received_ns: value.last_reference_received_ns,
            symbols,
        })
    }
}

impl From<(ReferenceSymbol, ReferenceSymbolSnapshot)> for WireSymbolTiming {
    fn from((symbol, value): (ReferenceSymbol, ReferenceSymbolSnapshot)) -> Self {
        Self {
            symbol: symbol as u8,
            candle_event_ns: value.candle_event_ns,
            candle_received_ns: value.candle_received_ns,
            aggregate_trade_event_ns: value.aggregate_trade_event_ns,
            aggregate_trade_received_ns: value.aggregate_trade_received_ns,
            book_ticker_received_ns: value.book_ticker_received_ns,
        }
    }
}

impl TryFrom<WireSymbolTiming> for (ReferenceSymbol, ReferenceSymbolSnapshot) {
    type Error = CodecError;

    fn try_from(value: WireSymbolTiming) -> Result<Self, Self::Error> {
        Ok((
            symbol(value.symbol)?,
            ReferenceSymbolSnapshot {
                candle_event_ns: value.candle_event_ns,
                candle_received_ns: value.candle_received_ns,
                aggregate_trade_event_ns: value.aggregate_trade_event_ns,
                aggregate_trade_received_ns: value.aggregate_trade_received_ns,
                book_ticker_received_ns: value.book_ticker_received_ns,
            },
        ))
    }
}

impl From<&SupervisorSnapshot> for WireSupervisor {
    fn from(value: &SupervisorSnapshot) -> Self {
        Self {
            mode: value.mode as u8,
            ready: value.ready,
            evaluated_at_ns: value.evaluated_at_ns,
            market_epoch: value.market_epoch,
            market_sequence: value.market_sequence,
            market_digest: value.market_digest,
            market_state_digest: value.market_state_digest,
            reference_epoch: value.reference_epoch,
            reference_sequence: value.reference_sequence,
            reference_digest: value.reference_digest,
            reference_state_digest: value.reference_state_digest,
            halt_reason: value.halt_reason.clone(),
            digest: value.digest,
        }
    }
}

impl TryFrom<WireSupervisor> for SupervisorSnapshot {
    type Error = CodecError;

    fn try_from(value: WireSupervisor) -> Result<Self, Self::Error> {
        Ok(Self {
            mode: supervisor_mode(value.mode)?,
            ready: value.ready,
            evaluated_at_ns: value.evaluated_at_ns,
            market_epoch: value.market_epoch,
            market_sequence: value.market_sequence,
            market_digest: value.market_digest,
            market_state_digest: value.market_state_digest,
            reference_epoch: value.reference_epoch,
            reference_sequence: value.reference_sequence,
            reference_digest: value.reference_digest,
            reference_state_digest: value.reference_state_digest,
            halt_reason: value.halt_reason,
            digest: value.digest,
        })
    }
}

impl From<(SessionKey, &SessionSourceState)> for WireSession {
    fn from((key, value): (SessionKey, &SessionSourceState)) -> Self {
        Self {
            asset: key.asset as u8,
            start_time_ms: key.start_time_ms,
            up_book: value.up_book.map(WireBook::from),
            down_book: value.down_book.map(WireBook::from),
            in_progress: value.in_progress.map(|candle| WireCandle::from(candle.0)),
            finalized: value.finalized.map(|candle| WireCandle::from(candle.0)),
        }
    }
}

impl TryFrom<WireSession> for (SessionKey, SessionSourceState) {
    type Error = CodecError;

    fn try_from(value: WireSession) -> Result<Self, Self::Error> {
        Ok((
            SessionKey {
                asset: asset(value.asset)?,
                start_time_ms: value.start_time_ms,
            },
            SessionSourceState {
                up_book: value.up_book.map(TryInto::try_into).transpose()?,
                down_book: value.down_book.map(TryInto::try_into).transpose()?,
                in_progress: value
                    .in_progress
                    .map(|candle| candle.try_into().map(InProgressCandle))
                    .transpose()?,
                finalized: value
                    .finalized
                    .map(|candle| candle.try_into().map(FinalizedCandle))
                    .transpose()?,
            },
        ))
    }
}

impl From<TokenBookView> for WireBook {
    fn from(value: TokenBookView) -> Self {
        Self {
            authoritative: value.authoritative,
            best_bid: value.best_bid.map(WireLevel::from),
            best_ask: value.best_ask.map(WireLevel::from),
        }
    }
}

impl TryFrom<WireBook> for TokenBookView {
    type Error = CodecError;

    fn try_from(value: WireBook) -> Result<Self, Self::Error> {
        Ok(Self {
            authoritative: value.authoritative,
            best_bid: value.best_bid.map(TryInto::try_into).transpose()?,
            best_ask: value.best_ask.map(TryInto::try_into).transpose()?,
        })
    }
}

impl From<(PriceMicros, QuantityMicros)> for WireLevel {
    fn from((price, quantity): (PriceMicros, QuantityMicros)) -> Self {
        Self {
            price_micros: price.as_micros(),
            quantity_micros: quantity.as_micros(),
        }
    }
}

impl TryFrom<WireLevel> for (PriceMicros, QuantityMicros) {
    type Error = CodecError;

    fn try_from(value: WireLevel) -> Result<Self, Self::Error> {
        Ok((
            PriceMicros::new(value.price_micros).map_err(|_| CodecError::Financial)?,
            QuantityMicros::new(value.quantity_micros).map_err(|_| CodecError::Financial)?,
        ))
    }
}

impl From<CandleData> for WireCandle {
    fn from(value: CandleData) -> Self {
        Self {
            symbol: value.symbol as u8,
            open_time_ms: value.open_time_ms,
            close_time_ms: value.close_time_ms,
            first_trade_id: value.first_trade_id,
            last_trade_id: value.last_trade_id,
            open_micros: value.open.as_micros(),
            high_micros: value.high.as_micros(),
            low_micros: value.low.as_micros(),
            close_micros: value.close.as_micros(),
            base_volume_e8: value.base_volume.as_e8(),
            quote_volume_e8: value.quote_volume.as_e8(),
            trade_count: value.trade_count,
        }
    }
}

impl TryFrom<WireCandle> for CandleData {
    type Error = CodecError;

    fn try_from(value: WireCandle) -> Result<Self, Self::Error> {
        Ok(Self {
            symbol: symbol(value.symbol)?,
            interval: CandleInterval::OneHourUtc,
            open_time_ms: value.open_time_ms,
            close_time_ms: value.close_time_ms,
            first_trade_id: value.first_trade_id,
            last_trade_id: value.last_trade_id,
            open: quote(value.open_micros)?,
            high: quote(value.high_micros)?,
            low: quote(value.low_micros)?,
            close: quote(value.close_micros)?,
            base_volume: reference_quantity(value.base_volume_e8)?,
            quote_volume: reference_quantity(value.quote_volume_e8)?,
            trade_count: value.trade_count,
        })
    }
}

fn quote(value: i64) -> Result<QuotePriceMicros, CodecError> {
    QuotePriceMicros::new(value).map_err(|_| CodecError::Financial)
}

fn reference_quantity(value: i64) -> Result<ReferenceQuantityE8, CodecError> {
    ReferenceQuantityE8::new(value).map_err(|_| CodecError::Financial)
}

fn asset(value: u8) -> Result<Asset, CodecError> {
    match value {
        0 => Ok(Asset::Bitcoin),
        1 => Ok(Asset::Ethereum),
        _ => Err(CodecError::Enum),
    }
}

fn symbol(value: u8) -> Result<ReferenceSymbol, CodecError> {
    match value {
        1 => Ok(ReferenceSymbol::BtcUsdt),
        2 => Ok(ReferenceSymbol::EthUsdt),
        _ => Err(CodecError::Enum),
    }
}

fn actor_mode_byte(value: ActorMode) -> u8 {
    match value {
        ActorMode::Starting => 1,
        ActorMode::CollectingSnapshots => 2,
        ActorMode::RecoveringSnapshot => 3,
        ActorMode::Ready => 4,
        ActorMode::Stale => 5,
        ActorMode::Inactive => 6,
        ActorMode::Shutdown => 7,
        ActorMode::Closed => 8,
        ActorMode::Halted => 9,
    }
}

fn actor_mode(value: u8) -> Result<ActorMode, CodecError> {
    match value {
        1 => Ok(ActorMode::Starting),
        2 => Ok(ActorMode::CollectingSnapshots),
        3 => Ok(ActorMode::RecoveringSnapshot),
        4 => Ok(ActorMode::Ready),
        5 => Ok(ActorMode::Stale),
        6 => Ok(ActorMode::Inactive),
        7 => Ok(ActorMode::Shutdown),
        8 => Ok(ActorMode::Closed),
        9 => Ok(ActorMode::Halted),
        _ => Err(CodecError::Enum),
    }
}

fn reference_health_byte(value: ReferenceHealth) -> u8 {
    match value {
        ReferenceHealth::Starting => 1,
        ReferenceHealth::Collecting => 2,
        ReferenceHealth::Ready => 3,
        ReferenceHealth::Disconnected => 4,
        ReferenceHealth::Shutdown => 5,
    }
}

fn reference_health(value: u8) -> Result<ReferenceHealth, CodecError> {
    match value {
        1 => Ok(ReferenceHealth::Starting),
        2 => Ok(ReferenceHealth::Collecting),
        3 => Ok(ReferenceHealth::Ready),
        4 => Ok(ReferenceHealth::Disconnected),
        5 => Ok(ReferenceHealth::Shutdown),
        _ => Err(CodecError::Enum),
    }
}

fn supervisor_mode(value: u8) -> Result<SupervisorMode, CodecError> {
    match value {
        1 => Ok(SupervisorMode::Starting),
        2 => Ok(SupervisorMode::Ready),
        3 => Ok(SupervisorMode::MarketUnavailable),
        4 => Ok(SupervisorMode::ReferenceUnavailable),
        5 => Ok(SupervisorMode::MarketStale),
        6 => Ok(SupervisorMode::ReferenceStale),
        7 => Ok(SupervisorMode::CrossFeedSkew),
        8 => Ok(SupervisorMode::SourceEventLag),
        9 => Ok(SupervisorMode::SourceEventFuture),
        10 => Ok(SupervisorMode::Halted),
        _ => Err(CodecError::Enum),
    }
}
