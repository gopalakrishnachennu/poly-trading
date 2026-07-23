use crate::payload::{
    encode_reference_payload, parse_combined_message, PayloadError, ReferenceEventKind,
    ReferenceSymbol, SOURCE_TIME_UNAVAILABLE_NS,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use futures_util::{SinkExt, StreamExt};
use market_recorder::{EventJournal, JournalBackendError};
use rustls::{ClientConfig, RootCertStore};
use serde_json::Value;
use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tokio::sync::mpsc;
use tokio::time::sleep;
use tokio_tungstenite::tungstenite::protocol::WebSocketConfig;
use tokio_tungstenite::tungstenite::{Error as WebSocketError, Message};
use tokio_tungstenite::{connect_async_tls_with_config, Connector};

pub const DEFAULT_REFERENCE_WS_ENDPOINT: &str = "wss://data-stream.binance.vision/stream?streams=btcusdt@kline_1h/ethusdt@kline_1h/btcusdt@aggTrade/ethusdt@aggTrade/btcusdt@bookTicker/ethusdt@bookTicker";
const SYSTEM_MARKET_ID: &str = "__reference_market_gateway__";
const EPOCH_START: &[u8] = b"REFERENCE_MARKET_EPOCH_START_V1";
const EPOCH_SYNCED: &[u8] = b"REFERENCE_MARKET_EPOCH_SYNCED_V1";
const EPOCH_SHUTDOWN: &[u8] = b"REFERENCE_MARKET_EPOCH_SHUTDOWN_V1";
const EPOCH_ROTATE: &[u8] = b"REFERENCE_MARKET_EPOCH_ROTATE_V1";
const EPOCH_DISCONNECTED: &[u8] = b"REFERENCE_MARKET_EPOCH_DISCONNECTED_V1";

#[derive(Clone, Debug)]
pub struct CaptureConfig {
    pub endpoint: String,
    pub rotation_interval: Duration,
    pub max_message_bytes: usize,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_REFERENCE_WS_ENDPOINT.to_owned(),
            rotation_interval: Duration::from_secs(23 * 60 * 60 + 50 * 60),
            max_message_bytes: 1024 * 1024,
        }
    }
}

impl CaptureConfig {
    fn validate(&self) -> Result<(), CaptureError> {
        if self.endpoint.is_empty()
            || self.rotation_interval.is_zero()
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
    Rotate,
    Disconnected,
}

#[derive(Debug, Error)]
pub enum CaptureError {
    #[error("invalid capture configuration")]
    InvalidConfig,
    #[error("websocket error: {0}")]
    WebSocket(#[from] WebSocketError),
    #[error("reference payload error: {0}")]
    Payload(#[from] PayloadError),
    #[error("event envelope error: {0}")]
    Envelope(#[from] SchemaError),
    #[error("journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("websocket message exceeds {limit} bytes")]
    MessageTooLarge { limit: usize },
    #[error("binary websocket messages are unsupported")]
    BinaryMessage,
    #[error("source sequence overflow")]
    SequenceOverflow,
    #[error("system clock is before Unix epoch")]
    ClockBeforeEpoch,
    #[error("system clock timestamp overflow")]
    ClockOverflow,
    #[error("event timestamp overflow")]
    TimestampOverflow,
    #[error("live-state channel is full")]
    LiveChannelFull,
    #[error("live-state channel is closed")]
    LiveChannelClosed,
}

/// Connects and captures one public reference-feed epoch.
///
/// # Errors
///
/// Returns [`CaptureError`] for invalid configuration, connection, decoding,
/// clock, envelope, or journal failures.
pub async fn capture_session<J: EventJournal>(
    config: &CaptureConfig,
    journal: &mut J,
    sequence: &mut u64,
) -> Result<CaptureOutcome, CaptureError> {
    capture_session_with_channel(config, journal, sequence, None).await
}

/// Captures one public feed epoch, journaling before bounded channel delivery.
///
/// Binance controls ping frames; tungstenite surfaces them and the gateway
/// returns a pong with the identical payload. A proactive rotation occurs ten
/// minutes before the documented 24-hour connection lifetime.
///
/// # Errors
///
/// Returns [`CaptureError`] for invalid configuration, connection, decoding,
/// clock, envelope, journal, or bounded-channel failures.
pub async fn capture_session_with_channel<J: EventJournal>(
    config: &CaptureConfig,
    journal: &mut J,
    sequence: &mut u64,
    live_sender: Option<&mpsc::Sender<EventEnvelope>>,
) -> Result<CaptureOutcome, CaptureError> {
    config.validate()?;
    let mut websocket_config = WebSocketConfig::default();
    websocket_config.max_message_size = Some(config.max_message_bytes);
    websocket_config.max_frame_size = Some(config.max_message_bytes);
    let (socket, _) = connect_async_tls_with_config(
        config.endpoint.as_str(),
        Some(websocket_config),
        false,
        Some(Connector::Rustls(reference_tls_config())),
    )
    .await?;
    let (mut writer, mut reader) = socket.split();
    journal_system(journal, sequence, EPOCH_START, live_sender)?;
    let rotation = sleep(config.rotation_interval);
    tokio::pin!(rotation);
    let mut seen = BTreeSet::new();
    let mut synchronized = false;

    let outcome: Result<CaptureOutcome, CaptureError> = async {
        loop {
            tokio::select! {
            shutdown = tokio::signal::ctrl_c() => {
                if shutdown.is_ok() {
                    journal_system(journal, sequence, EPOCH_SHUTDOWN, live_sender)?;
                    let _ = writer.send(Message::Close(None)).await;
                    return Ok(CaptureOutcome::Shutdown);
                }
                journal_system(journal, sequence, EPOCH_DISCONNECTED, live_sender)?;
                return Ok(CaptureOutcome::Disconnected);
            }
            () = &mut rotation => {
                journal_system(journal, sequence, EPOCH_ROTATE, live_sender)?;
                let _ = writer.send(Message::Close(None)).await;
                return Ok(CaptureOutcome::Rotate);
            }
            message = reader.next() => {
                let Some(message) = message else {
                    journal_system(journal, sequence, EPOCH_DISCONNECTED, live_sender)?;
                    return Ok(CaptureOutcome::Disconnected);
                };
                match message {
                    Ok(Message::Text(text)) => {
                        if text.len() > config.max_message_bytes { return Err(CaptureError::MessageTooLarge { limit: config.max_message_bytes }); }
                        if is_server_shutdown(text.as_str()) {
                            journal_system(journal, sequence, EPOCH_DISCONNECTED, live_sender)?;
                            return Ok(CaptureOutcome::Disconnected);
                        }
                        let received_time_ns = now_ns()?;
                        let decoded = parse_combined_message(text.as_str())?;
                        let kind = decoded.event.kind();
                        let symbol = decoded.event.symbol();
                        let event_time_ns = decoded.event_time_ms.map_or(Ok(SOURCE_TIME_UNAVAILABLE_NS), |ms| ms.checked_mul(1_000_000).ok_or(CaptureError::TimestampOverflow))?;
                        let envelope = EventEnvelope::new(EventSource::ReferencePrice, take_sequence(sequence)?, event_time_ns, received_time_ns, format!("BINANCE_SPOT:{}", symbol.as_upper()), encode_reference_payload(decoded)?)?;
                        append_and_deliver(journal, &envelope, live_sender)?;
                        seen.insert((symbol, readiness_kind(kind)));
                        if !synchronized && ready(&seen) {
                            journal_system(journal, sequence, EPOCH_SYNCED, live_sender)?;
                            synchronized = true;
                        }
                    }
                    Ok(Message::Ping(payload)) => writer.send(Message::Pong(payload)).await?,
                    Ok(Message::Pong(_) | Message::Frame(_)) => {}
                    Ok(Message::Close(_)) | Err(_) => {
                        journal_system(journal, sequence, EPOCH_DISCONNECTED, live_sender)?;
                        return Ok(CaptureOutcome::Disconnected);
                    }
                    Ok(Message::Binary(_)) => return Err(CaptureError::BinaryMessage),
                }
            }
            }
        }
    }
    .await;
    if outcome.is_err() {
        // Any failure after EPOCH_START must close that durable recovery epoch.
        // If this append itself fails, the journal error is the safer result.
        journal_system(journal, sequence, EPOCH_DISCONNECTED, live_sender)?;
    }
    outcome
}

fn reference_tls_config() -> Arc<ClientConfig> {
    let mut roots = RootCertStore::empty();
    roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let mut config = ClientConfig::builder_with_protocol_versions(&[&rustls::version::TLS12])
        .with_root_certificates(roots)
        .with_no_client_auth();
    config.alpn_protocols = vec![b"http/1.1".to_vec()];
    Arc::new(config)
}

fn readiness_kind(kind: ReferenceEventKind) -> u8 {
    match kind {
        ReferenceEventKind::InProgressCandle | ReferenceEventKind::FinalizedCandle => 1,
        ReferenceEventKind::AggregateTrade => 2,
        ReferenceEventKind::BookTicker => 3,
    }
}
fn ready(seen: &BTreeSet<(ReferenceSymbol, u8)>) -> bool {
    ReferenceSymbol::ALL
        .iter()
        .all(|symbol| (1..=3).all(|kind| seen.contains(&(*symbol, kind))))
}
fn is_server_shutdown(text: &str) -> bool {
    serde_json::from_str::<Value>(text)
        .ok()
        .and_then(|root| root.get("data").cloned().or(Some(root)))
        .and_then(|data| data.get("e").and_then(Value::as_str).map(str::to_owned))
        .is_some_and(|event| event == "serverShutdown")
}
fn journal_system<J: EventJournal>(
    journal: &mut J,
    sequence: &mut u64,
    payload: &[u8],
    live_sender: Option<&mpsc::Sender<EventEnvelope>>,
) -> Result<(), CaptureError> {
    let now = now_ns()?;
    let event = EventEnvelope::new(
        EventSource::System,
        take_sequence(sequence)?,
        now,
        now,
        SYSTEM_MARKET_ID.to_owned(),
        payload.to_vec(),
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
fn take_sequence(sequence: &mut u64) -> Result<u64, CaptureError> {
    let current = *sequence;
    *sequence = sequence
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
    use market_recorder::{scan_path, JournalWriter};
    use tempfile::tempdir;

    #[test]
    fn readiness_requires_all_three_feeds_for_both_symbols() {
        let mut seen = BTreeSet::new();
        for symbol in ReferenceSymbol::ALL {
            for kind in 1..=3 {
                seen.insert((symbol, kind));
            }
        }
        assert!(ready(&seen));
        seen.remove(&(ReferenceSymbol::EthUsdt, 2));
        assert!(!ready(&seen));
    }

    #[test]
    fn server_shutdown_is_recognized_in_wrapped_or_raw_form() {
        assert!(is_server_shutdown(
            r#"{"stream":"x","data":{"e":"serverShutdown","E":1}}"#
        ));
        assert!(is_server_shutdown(r#"{"e":"serverShutdown","E":1}"#));
        assert!(!is_server_shutdown(r#"{"e":"aggTrade"}"#));
    }

    #[test]
    fn reference_tls_explicitly_negotiates_http_one_websocket_upgrade() {
        assert_eq!(reference_tls_config().alpn_protocols, [b"http/1.1"]);
    }

    #[test]
    fn bounded_delivery_is_journal_first() {
        let dir = tempdir().expect("dir");
        let path = dir.path().join("reference.journal");
        let mut journal = JournalWriter::open(&path).expect("journal");
        let (sender, _receiver) = mpsc::channel(1);
        let event = EventEnvelope::new(
            EventSource::System,
            1,
            1,
            1,
            SYSTEM_MARKET_ID.to_owned(),
            b"one".to_vec(),
        )
        .expect("event");
        sender.try_send(event.clone()).expect("fill");
        let second = EventEnvelope::new(
            EventSource::System,
            2,
            2,
            2,
            SYSTEM_MARKET_ID.to_owned(),
            b"two".to_vec(),
        )
        .expect("event");
        assert!(matches!(
            append_and_deliver(&mut journal, &second, Some(&sender)),
            Err(CaptureError::LiveChannelFull)
        ));
        assert_eq!(scan_path(path).expect("scan").events, vec![second]);
    }
}
