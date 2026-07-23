//! Public CLOB market WebSocket subscription, validation, and journaling.

use crate::domain::{validate_hex_id, validate_token_id, MarketIdentity};
use crate::payload::{PayloadError, PublicEventKind, PUBLIC_EVENT_PAYLOAD_VERSION};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use futures_util::{SinkExt, StreamExt};
use market_recorder::{EventJournal, JournalBackendError, JournalError};
use serde::Serialize;
use serde_json::{Map, Value};
use std::collections::{HashMap, HashSet};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::time::{interval, sleep, MissedTickBehavior};
use tokio_tungstenite::connect_async_with_config;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error as WebSocketError, Message};

pub const DEFAULT_MARKET_WS_ENDPOINT: &str = "wss://ws-subscriptions-clob.polymarket.com/ws/market";
const MAX_EVENTS_PER_MESSAGE: usize = 512;
const MAX_ASSETS_PER_EVENT: usize = 1_024;
const SYSTEM_MARKET_ID: &str = "__public_market_gateway__";

#[derive(Clone, Debug)]
pub struct CaptureConfig {
    pub endpoint: String,
    pub heartbeat_interval: Duration,
    pub pong_timeout: Duration,
    pub rediscovery_interval: Duration,
    pub max_message_bytes: usize,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_MARKET_WS_ENDPOINT.to_owned(),
            heartbeat_interval: Duration::from_secs(10),
            pong_timeout: Duration::from_secs(30),
            rediscovery_interval: Duration::from_secs(60),
            max_message_bytes: 16 * 1024 * 1024,
        }
    }
}

impl CaptureConfig {
    fn validate(&self) -> Result<(), CaptureError> {
        if self.endpoint.is_empty()
            || self.heartbeat_interval.is_zero()
            || self.pong_timeout <= self.heartbeat_interval
            || self.rediscovery_interval.is_zero()
            || self.max_message_bytes == 0
        {
            return Err(CaptureError::InvalidConfig);
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaptureOutcome {
    Shutdown,
    Rediscover,
}

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("invalid capture configuration")]
    InvalidConfig,
    #[error("no market identities supplied")]
    EmptySubscription,
    #[error("token {token} is assigned to multiple conditions")]
    ConflictingToken { token: String },
    #[error("websocket error: {0}")]
    WebSocket(#[from] WebSocketError),
    #[error("websocket message exceeds {limit} bytes")]
    MessageTooLarge { limit: usize },
    #[error("invalid websocket JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("unsupported websocket message root")]
    InvalidRoot,
    #[error("websocket message contains too many events")]
    TooManyEvents,
    #[error("missing or invalid websocket field: {0}")]
    InvalidField(&'static str),
    #[error("unknown websocket event type: {0}")]
    UnknownEventType(String),
    #[error("public event payload error: {0}")]
    Payload(#[from] PayloadError),
    #[error("websocket event has too many assets")]
    TooManyAssets,
    #[error("event for unsubscribed condition: {0}")]
    UnsubscribedCondition(String),
    #[error("event token {token} does not belong to condition {condition}")]
    UnsubscribedAsset { token: String, condition: String },
    #[error("invalid condition or token identifier: {0}")]
    Identity(#[from] crate::domain::IdentityError),
    #[error("timestamp overflow")]
    TimestampOverflow,
    #[error("source sequence overflow")]
    SequenceOverflow,
    #[error("normalized event payload is too large")]
    PayloadTooLarge,
    #[error("event envelope error: {0}")]
    Envelope(#[from] SchemaError),
    #[error("journal error: {0}")]
    Journal(#[from] JournalError),
    #[error("journal backend error: {0}")]
    JournalBackend(#[from] JournalBackendError),
    #[error("live-state channel is full")]
    LiveChannelFull,
    #[error("live-state channel is closed")]
    LiveChannelClosed,
    #[error("heartbeat timed out")]
    HeartbeatTimeout,
    #[error("websocket closed by peer")]
    Closed,
    #[error("system clock is before the Unix epoch")]
    ClockBeforeEpoch,
    #[error("system clock timestamp overflow")]
    ClockOverflow,
}

#[derive(Debug)]
struct ParsedMarketEvent {
    kind: PublicEventKind,
    condition_id: String,
    asset_ids: Vec<String>,
    timestamp_ms: i64,
    canonical_json: Vec<u8>,
}

impl ParsedMarketEvent {
    fn encode_payload(&self) -> Result<Vec<u8>, CaptureError> {
        let asset_count =
            u16::try_from(self.asset_ids.len()).map_err(|_| CaptureError::TooManyAssets)?;
        let json_length =
            u32::try_from(self.canonical_json.len()).map_err(|_| CaptureError::PayloadTooLarge)?;
        let mut capacity = 2_usize + 1 + 1 + 8 + 2 + 4 + self.canonical_json.len();
        for asset in &self.asset_ids {
            capacity = capacity
                .checked_add(2)
                .and_then(|value| value.checked_add(asset.len()))
                .ok_or(CaptureError::PayloadTooLarge)?;
        }

        let mut payload = Vec::with_capacity(capacity);
        payload.extend_from_slice(&PUBLIC_EVENT_PAYLOAD_VERSION.to_le_bytes());
        payload.push(self.kind as u8);
        payload.push(0);
        payload.extend_from_slice(&self.timestamp_ms.to_le_bytes());
        payload.extend_from_slice(&asset_count.to_le_bytes());
        for asset in &self.asset_ids {
            let length = u16::try_from(asset.len()).map_err(|_| CaptureError::PayloadTooLarge)?;
            payload.extend_from_slice(&length.to_le_bytes());
            payload.extend_from_slice(asset.as_bytes());
        }
        payload.extend_from_slice(&json_length.to_le_bytes());
        payload.extend_from_slice(&self.canonical_json);
        Ok(payload)
    }

    fn envelope(
        &self,
        sequence: u64,
        received_time_ns: i64,
    ) -> Result<EventEnvelope, CaptureError> {
        let event_time_ns = self
            .timestamp_ms
            .checked_mul(1_000_000)
            .ok_or(CaptureError::TimestampOverflow)?;
        Ok(EventEnvelope::new(
            EventSource::Market,
            sequence,
            event_time_ns,
            received_time_ns,
            self.condition_id.clone(),
            self.encode_payload()?,
        )?)
    }
}

#[derive(Debug)]
struct SubscriptionGuard {
    assets: Vec<String>,
    asset_conditions: HashMap<String, String>,
    conditions: HashSet<String>,
}

impl SubscriptionGuard {
    fn new(markets: &[MarketIdentity]) -> Result<Self, CaptureError> {
        if markets.is_empty() {
            return Err(CaptureError::EmptySubscription);
        }
        let mut asset_conditions = HashMap::new();
        let mut conditions = HashSet::new();
        for market in markets {
            conditions.insert(market.condition_id.clone());
            for token in market.token_ids() {
                if let Some(existing) =
                    asset_conditions.insert(token.to_owned(), market.condition_id.clone())
                {
                    if existing != market.condition_id {
                        return Err(CaptureError::ConflictingToken {
                            token: token.to_owned(),
                        });
                    }
                }
            }
        }
        let mut assets: Vec<_> = asset_conditions.keys().cloned().collect();
        assets.sort();
        Ok(Self {
            assets,
            asset_conditions,
            conditions,
        })
    }

    fn subscription_json(&self) -> Result<String, CaptureError> {
        #[derive(Serialize)]
        struct Subscription<'a> {
            assets_ids: &'a [String],
            #[serde(rename = "type")]
            channel_type: &'static str,
            custom_feature_enabled: bool,
        }
        serde_json::to_string(&Subscription {
            assets_ids: &self.assets,
            channel_type: "market",
            custom_feature_enabled: false,
        })
        .map_err(CaptureError::Json)
    }

    fn validate_event(&self, event: &ParsedMarketEvent) -> Result<(), CaptureError> {
        if !self.conditions.contains(&event.condition_id) {
            return Err(CaptureError::UnsubscribedCondition(
                event.condition_id.clone(),
            ));
        }
        for token in &event.asset_ids {
            match self.asset_conditions.get(token) {
                Some(condition) if condition == &event.condition_id => {}
                _ => {
                    return Err(CaptureError::UnsubscribedAsset {
                        token: token.clone(),
                        condition: event.condition_id.clone(),
                    });
                }
            }
        }
        Ok(())
    }
}

/// Connects one synchronization epoch and journals validated public events.
///
/// A disconnect is returned as an error so the caller can rediscover market
/// identity before reconnecting. A fresh `book` snapshot for every subscribed
/// token causes an explicit synchronized system event to be journaled.
///
/// # Errors
///
/// Returns [`CaptureError`] for configuration, connection, heartbeat, schema,
/// subscription, clock, or journal failures.
pub async fn capture_session<J: EventJournal>(
    config: &CaptureConfig,
    markets: &[MarketIdentity],
    journal: &mut J,
    sequence: &mut u64,
) -> Result<CaptureOutcome, CaptureError> {
    capture_session_with_channel(config, markets, journal, sequence, None).await
}

/// Connects one synchronization epoch, journals validated public events, and
/// optionally delivers the identical envelopes to a bounded live-state channel.
///
/// Journal append always occurs before channel delivery. Full or closed
/// channels fail the capture epoch; no event is dropped silently.
///
/// # Errors
///
/// Returns [`CaptureError`] for configuration, connection, heartbeat, schema,
/// subscription, clock, journal, or bounded-channel failures.
#[allow(clippy::too_many_lines)]
pub async fn capture_session_with_channel<J: EventJournal>(
    config: &CaptureConfig,
    markets: &[MarketIdentity],
    journal: &mut J,
    sequence: &mut u64,
    live_sender: Option<&mpsc::Sender<EventEnvelope>>,
) -> Result<CaptureOutcome, CaptureError> {
    config.validate()?;
    let guard = SubscriptionGuard::new(markets)?;
    let mut websocket_config = WebSocketConfig::default();
    websocket_config.max_message_size = Some(config.max_message_bytes);
    websocket_config.max_frame_size = Some(config.max_message_bytes);
    let (socket, _) =
        connect_async_with_config(config.endpoint.as_str(), Some(websocket_config), false).await?;
    let (mut writer, mut reader) = socket.split();
    writer
        .send(Message::Text(guard.subscription_json()?.into()))
        .await?;

    journal_system_event(
        journal,
        sequence,
        b"PUBLIC_MARKET_EPOCH_START_V1",
        live_sender,
    )?;
    for market in markets {
        journal_identity_event(journal, sequence, market, live_sender)?;
    }
    let mut pending_snapshots: HashSet<String> = guard.assets.iter().cloned().collect();
    let mut synchronized = false;
    let mut last_pong = Instant::now();
    let mut heartbeat = interval(config.heartbeat_interval);
    heartbeat.set_missed_tick_behavior(MissedTickBehavior::Delay);
    heartbeat.tick().await;
    let rediscovery = sleep(config.rediscovery_interval);
    tokio::pin!(rediscovery);

    loop {
        tokio::select! {
            shutdown = tokio::signal::ctrl_c() => {
                shutdown.map_err(|_| CaptureError::Closed)?;
                journal_system_event(
                    journal,
                    sequence,
                    b"PUBLIC_MARKET_EPOCH_SHUTDOWN_V1",
                    live_sender,
                )?;
                return Ok(CaptureOutcome::Shutdown);
            }
            () = &mut rediscovery => {
                journal_system_event(
                    journal,
                    sequence,
                    b"PUBLIC_MARKET_EPOCH_REDISCOVER_V1",
                    live_sender,
                )?;
                return Ok(CaptureOutcome::Rediscover);
            }
            _ = heartbeat.tick() => {
                if last_pong.elapsed() > config.pong_timeout {
                    return Err(CaptureError::HeartbeatTimeout);
                }
                writer.send(Message::Text("PING".into())).await?;
            }
            message = reader.next() => {
                let message = message.ok_or(CaptureError::Closed)??;
                match message {
                    Message::Text(text) if text.as_str() == "PONG" => {
                        last_pong = Instant::now();
                    }
                    Message::Text(text) => {
                        if text.len() > config.max_message_bytes {
                            return Err(CaptureError::MessageTooLarge {
                                limit: config.max_message_bytes,
                            });
                        }
                        let received_time_ns = now_ns()?;
                        for event in parse_market_messages(text.as_str(), &guard)? {
                            if event.kind == PublicEventKind::Book {
                                for asset in &event.asset_ids {
                                    pending_snapshots.remove(asset);
                                }
                            }
                            let current = take_sequence(sequence)?;
                            let envelope = event.envelope(current, received_time_ns)?;
                            append_and_deliver(journal, &envelope, live_sender)?;
                        }
                        if !synchronized && pending_snapshots.is_empty() {
                            journal_system_event(
                                journal,
                                sequence,
                                b"PUBLIC_MARKET_EPOCH_SYNCED_V1",
                                live_sender,
                            )?;
                            synchronized = true;
                        }
                    }
                    Message::Ping(payload) => {
                        writer.send(Message::Pong(payload)).await?;
                    }
                    Message::Pong(_) => {
                        last_pong = Instant::now();
                    }
                    Message::Close(_) => return Err(CaptureError::Closed),
                    Message::Binary(_) => return Err(CaptureError::InvalidRoot),
                    Message::Frame(_) => {}
                }
            }
        }
    }
}

fn parse_market_messages(
    text: &str,
    guard: &SubscriptionGuard,
) -> Result<Vec<ParsedMarketEvent>, CaptureError> {
    let value: Value = serde_json::from_str(text)?;
    let objects: Vec<&Map<String, Value>> = match &value {
        Value::Object(object) => vec![object],
        Value::Array(values) => {
            if values.len() > MAX_EVENTS_PER_MESSAGE {
                return Err(CaptureError::TooManyEvents);
            }
            values
                .iter()
                .map(|value| value.as_object().ok_or(CaptureError::InvalidRoot))
                .collect::<Result<_, _>>()?
        }
        _ => return Err(CaptureError::InvalidRoot),
    };

    let mut events = Vec::with_capacity(objects.len());
    for object in objects {
        let kind = PublicEventKind::parse(string_field(object, "event_type")?)?;
        let condition_id = string_field(object, "market")?.to_owned();
        validate_hex_id("market", &condition_id)?;
        let timestamp_ms = timestamp_field(object, "timestamp")?;
        let asset_ids = asset_ids(object, kind)?;
        if asset_ids.is_empty() || asset_ids.len() > MAX_ASSETS_PER_EVENT {
            return Err(CaptureError::TooManyAssets);
        }
        for asset in &asset_ids {
            validate_token_id(asset)?;
        }
        let canonical_json = serde_json::to_vec(object)?;
        let event = ParsedMarketEvent {
            kind,
            condition_id,
            asset_ids,
            timestamp_ms,
            canonical_json,
        };
        guard.validate_event(&event)?;
        events.push(event);
    }
    Ok(events)
}

fn asset_ids(
    object: &Map<String, Value>,
    kind: PublicEventKind,
) -> Result<Vec<String>, CaptureError> {
    if kind == PublicEventKind::PriceChange {
        let changes = object
            .get("price_changes")
            .and_then(Value::as_array)
            .ok_or(CaptureError::InvalidField("price_changes"))?;
        if changes.len() > MAX_ASSETS_PER_EVENT {
            return Err(CaptureError::TooManyAssets);
        }
        changes
            .iter()
            .map(|change| {
                change
                    .as_object()
                    .ok_or(CaptureError::InvalidField("price_changes[]"))
                    .and_then(|change| string_field(change, "asset_id"))
                    .map(ToOwned::to_owned)
            })
            .collect()
    } else {
        Ok(vec![string_field(object, "asset_id")?.to_owned()])
    }
}

fn string_field<'a>(
    object: &'a Map<String, Value>,
    field: &'static str,
) -> Result<&'a str, CaptureError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(CaptureError::InvalidField(field))
}

fn timestamp_field(object: &Map<String, Value>, field: &'static str) -> Result<i64, CaptureError> {
    let value = object.get(field).ok_or(CaptureError::InvalidField(field))?;
    let timestamp = match value {
        Value::String(value) => value
            .parse::<i64>()
            .map_err(|_| CaptureError::InvalidField(field)),
        Value::Number(value) => value.as_i64().ok_or(CaptureError::InvalidField(field)),
        _ => Err(CaptureError::InvalidField(field)),
    }?;
    if timestamp < 0 {
        return Err(CaptureError::InvalidField(field));
    }
    Ok(timestamp)
}

fn journal_system_event<J: EventJournal>(
    journal: &mut J,
    sequence: &mut u64,
    payload: &[u8],
    live_sender: Option<&mpsc::Sender<EventEnvelope>>,
) -> Result<(), CaptureError> {
    let timestamp = now_ns()?;
    let current = take_sequence(sequence)?;
    let event = EventEnvelope::new(
        EventSource::System,
        current,
        timestamp,
        timestamp,
        SYSTEM_MARKET_ID.to_owned(),
        payload.to_vec(),
    )?;
    append_and_deliver(journal, &event, live_sender)?;
    journal.sync_events()?;
    Ok(())
}

/// Stores the immutable market contract once per capture epoch so later
/// research can bind raw ticks to the exact hourly market and resolution rules.
fn journal_identity_event<J: EventJournal>(
    journal: &mut J,
    sequence: &mut u64,
    market: &MarketIdentity,
    live_sender: Option<&mpsc::Sender<EventEnvelope>>,
) -> Result<(), CaptureError> {
    #[derive(Serialize)]
    struct IdentityPayload<'a> {
        schema_version: u16,
        event_type: &'static str,
        asset: &'a str,
        event_id: &'a str,
        market_id: &'a str,
        condition_id: &'a str,
        question_id: &'a str,
        event_slug: &'a str,
        market_slug: &'a str,
        series_id: &'a str,
        series_slug: &'a str,
        title: &'a str,
        start_time_ms: i64,
        end_time_ms: i64,
        resolution_source: &'a str,
        description: &'a str,
        up_token_id: &'a str,
        down_token_id: &'a str,
        rules_fingerprint_hex: String,
    }
    let payload = serde_json::to_vec(&IdentityPayload {
        schema_version: 1,
        event_type: "market_identity",
        asset: market.asset.as_str(),
        event_id: &market.event_id,
        market_id: &market.market_id,
        condition_id: &market.condition_id,
        question_id: &market.question_id,
        event_slug: &market.event_slug,
        market_slug: &market.market_slug,
        series_id: &market.series_id,
        series_slug: &market.series_slug,
        title: &market.title,
        start_time_ms: market.start_time_ms,
        end_time_ms: market.end_time_ms,
        resolution_source: &market.resolution_source,
        description: &market.description,
        up_token_id: &market.up_token_id,
        down_token_id: &market.down_token_id,
        rules_fingerprint_hex: hex(&market.rules_fingerprint),
    })
    .map_err(CaptureError::Json)?;
    let timestamp = now_ns()?;
    let current = take_sequence(sequence)?;
    let event = EventEnvelope::new(
        EventSource::System,
        current,
        timestamp,
        timestamp,
        market.condition_id.clone(),
        payload,
    )?;
    append_and_deliver(journal, &event, live_sender)?;
    journal.sync_events()?;
    Ok(())
}

fn append_and_deliver<J: EventJournal>(
    journal: &mut J,
    event: &EventEnvelope,
    live_sender: Option<&mpsc::Sender<EventEnvelope>>,
) -> Result<(), CaptureError> {
    journal.append_event(event)?;
    if let Some(sender) = live_sender {
        sender
            .try_send(event.clone())
            .map_err(|error| match error {
                mpsc::error::TrySendError::Full(_) => CaptureError::LiveChannelFull,
                mpsc::error::TrySendError::Closed(_) => CaptureError::LiveChannelClosed,
            })?;
    }
    Ok(())
}

fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(char::from(DIGITS[usize::from(byte >> 4)]));
        output.push(char::from(DIGITS[usize::from(byte & 0x0f)]));
    }
    output
}

fn take_sequence(sequence: &mut u64) -> Result<u64, CaptureError> {
    let current = *sequence;
    *sequence = (*sequence)
        .checked_add(1)
        .ok_or(CaptureError::SequenceOverflow)?;
    Ok(current)
}

fn now_ns() -> Result<i64, CaptureError> {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_err(|_| CaptureError::ClockBeforeEpoch)?;
    i64::try_from(duration.as_nanos()).map_err(|_| CaptureError::ClockOverflow)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::Asset;
    use market_recorder::{scan_path, JournalWriter};
    use tempfile::tempdir;

    fn identity(asset: Asset, marker: char, up: &str, down: &str) -> MarketIdentity {
        MarketIdentity {
            asset,
            event_id: format!("event-{marker}"),
            market_id: format!("market-{marker}"),
            condition_id: format!("0x{}", marker.to_string().repeat(64)),
            question_id: format!("0x{}", "f".repeat(64)),
            event_slug: format!("event-{marker}"),
            market_slug: format!("market-{marker}"),
            series_id: "series".to_owned(),
            series_slug: "hourly".to_owned(),
            title: "Up or Down".to_owned(),
            start_time_ms: 1,
            end_time_ms: 3_600_001,
            resolution_source: "source".to_owned(),
            description: "rules".to_owned(),
            up_token_id: up.to_owned(),
            down_token_id: down.to_owned(),
            rules_fingerprint: [0; 32],
        }
    }

    fn guard() -> SubscriptionGuard {
        SubscriptionGuard::new(&[identity(Asset::Bitcoin, 'a', "11", "22")]).expect("guard")
    }

    #[test]
    fn subscription_is_deterministic_and_contains_no_auth() {
        let guard = guard();
        let first = guard.subscription_json().expect("subscription");
        let second = guard.subscription_json().expect("subscription");
        assert_eq!(first, second);
        assert!(!first.contains("auth"));
        assert_eq!(
            first,
            r#"{"assets_ids":["11","22"],"type":"market","custom_feature_enabled":false}"#
        );
    }

    #[test]
    fn parses_object_and_array_messages() {
        let condition = format!("0x{}", "a".repeat(64));
        let book = format!(
            r#"{{"event_type":"book","asset_id":"11","market":"{condition}","bids":[],"asks":[],"timestamp":"1000","hash":"0x1"}}"#
        );
        let parsed = parse_market_messages(&book, &guard()).expect("book");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].kind, PublicEventKind::Book);

        let array = format!("[{book},{book}]");
        assert_eq!(
            parse_market_messages(&array, &guard())
                .expect("array")
                .len(),
            2
        );
    }

    #[test]
    fn rejects_cross_market_token() {
        let condition = format!("0x{}", "a".repeat(64));
        let message = format!(
            r#"{{"event_type":"book","asset_id":"999","market":"{condition}","bids":[],"asks":[],"timestamp":"1000"}}"#
        );
        assert!(matches!(
            parse_market_messages(&message, &guard()),
            Err(CaptureError::UnsubscribedAsset { .. })
        ));
    }

    #[test]
    fn rejects_negative_or_non_numeric_timestamps() {
        let condition = format!("0x{}", "a".repeat(64));
        for timestamp in [r"-1", r#""invalid""#] {
            let message = format!(
                r#"{{"event_type":"book","asset_id":"11","market":"{condition}","bids":[],"asks":[],"timestamp":{timestamp}}}"#
            );
            assert!(matches!(
                parse_market_messages(&message, &guard()),
                Err(CaptureError::InvalidField("timestamp"))
            ));
        }
    }

    #[test]
    fn normalized_event_round_trips_through_journal() {
        let condition = format!("0x{}", "a".repeat(64));
        let message = format!(
            r#"{{"event_type":"last_trade_price","asset_id":"11","market":"{condition}","price":"0.5","side":"BUY","size":"1","timestamp":"1000"}}"#
        );
        let event = parse_market_messages(&message, &guard())
            .expect("parse")
            .remove(0);
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("public.journal");
        let mut writer = JournalWriter::open(&path).expect("writer");
        writer
            .append(&event.envelope(7, 1_100_000_000).expect("envelope"))
            .expect("append");
        writer.sync().expect("sync");
        drop(writer);

        let report = scan_path(path).expect("scan");
        assert_eq!(report.events.len(), 1);
        assert_eq!(report.events[0].market_id, condition);
        assert_eq!(report.events[0].event_time_ns, 1_000_000_000);
    }

    #[test]
    fn journals_immutable_market_identity_for_tick_research() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("identity.journal");
        let mut writer = JournalWriter::open(&path).expect("writer");
        let mut sequence = 0;
        let market = identity(Asset::Bitcoin, 'a', "11", "22");
        journal_identity_event(&mut writer, &mut sequence, &market, None).expect("identity");
        drop(writer);
        let report = scan_path(path).expect("scan");
        assert_eq!(report.events.len(), 1);
        assert_eq!(report.events[0].market_id, market.condition_id);
        let payload: Value = serde_json::from_slice(&report.events[0].payload).expect("json");
        assert_eq!(payload["event_type"], "market_identity");
        assert_eq!(payload["up_token_id"], "11");
        assert_eq!(payload["down_token_id"], "22");
        assert_eq!(payload["rules_fingerprint_hex"], "00".repeat(32));
    }

    #[test]
    fn full_live_channel_fails_after_journal_append() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("full-channel.journal");
        let mut writer = JournalWriter::open(&path).expect("writer");
        let (sender, _receiver) = mpsc::channel(1);
        let blocker = EventEnvelope::new(
            EventSource::System,
            0,
            1,
            1,
            SYSTEM_MARKET_ID.to_owned(),
            b"blocker".to_vec(),
        )
        .expect("blocker");
        sender.try_send(blocker).expect("fill channel");
        let journaled = EventEnvelope::new(
            EventSource::System,
            1,
            2,
            2,
            SYSTEM_MARKET_ID.to_owned(),
            b"journaled".to_vec(),
        )
        .expect("journaled");

        assert!(matches!(
            append_and_deliver(&mut writer, &journaled, Some(&sender)),
            Err(CaptureError::LiveChannelFull)
        ));
        let report = scan_path(path).expect("scan");
        assert_eq!(report.events, vec![journaled]);
    }

    #[test]
    fn closed_live_channel_fails_after_journal_append() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("closed-channel.journal");
        let mut writer = JournalWriter::open(&path).expect("writer");
        let (sender, receiver) = mpsc::channel(1);
        drop(receiver);
        let journaled = EventEnvelope::new(
            EventSource::System,
            0,
            1,
            1,
            SYSTEM_MARKET_ID.to_owned(),
            b"journaled".to_vec(),
        )
        .expect("journaled");

        assert!(matches!(
            append_and_deliver(&mut writer, &journaled, Some(&sender)),
            Err(CaptureError::LiveChannelClosed)
        ));
        let report = scan_path(path).expect("scan");
        assert_eq!(report.events, vec![journaled]);
    }
}
