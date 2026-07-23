use crate::payload::{
    decode_reference_payload, AggregateTrade, BookTicker, CandleData, FinalizedCandle,
    InProgressCandle, PayloadError, ReferenceEvent, ReferenceSymbol, SOURCE_TIME_UNAVAILABLE_NS,
};
use event_schema::{EventEnvelope, EventSource};
use market_recorder::{JournalError, JournalReader, JournalTail};
use std::collections::BTreeMap;
use std::path::Path;
use thiserror::Error;

const SYSTEM_MARKET_ID: &str = "__reference_market_gateway__";
const EPOCH_START: &[u8] = b"REFERENCE_MARKET_EPOCH_START_V1";
const EPOCH_SYNCED: &[u8] = b"REFERENCE_MARKET_EPOCH_SYNCED_V1";
const EPOCH_SHUTDOWN: &[u8] = b"REFERENCE_MARKET_EPOCH_SHUTDOWN_V1";
const EPOCH_ROTATE: &[u8] = b"REFERENCE_MARKET_EPOCH_ROTATE_V1";
const EPOCH_DISCONNECTED: &[u8] = b"REFERENCE_MARKET_EPOCH_DISCONNECTED_V1";

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReferenceHealth {
    Starting,
    Collecting,
    Ready,
    Disconnected,
    Shutdown,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
struct SymbolState {
    in_progress: Option<InProgressCandle>,
    last_finalized: Option<FinalizedCandle>,
    aggregate_trade: Option<AggregateTrade>,
    book_ticker: Option<BookTicker>,
    candle_event_ns: Option<i64>,
    candle_received_ns: Option<i64>,
    aggregate_trade_event_ns: Option<i64>,
    aggregate_trade_received_ns: Option<i64>,
    book_ticker_received_ns: Option<i64>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReferenceSymbolSnapshot {
    pub candle_event_ns: Option<i64>,
    pub candle_received_ns: Option<i64>,
    pub aggregate_trade_event_ns: Option<i64>,
    pub aggregate_trade_received_ns: Option<i64>,
    pub book_ticker_received_ns: Option<i64>,
}

impl ReferenceSymbolSnapshot {
    #[must_use]
    pub fn oldest_required_received_ns(self) -> Option<i64> {
        Some(
            self.candle_received_ns?
                .min(self.aggregate_trade_received_ns?)
                .min(self.book_ticker_received_ns?),
        )
    }

    #[must_use]
    pub fn latest_source_event_ns(self) -> Option<i64> {
        Some(self.candle_event_ns?.max(self.aggregate_trade_event_ns?))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferenceSnapshot {
    pub health: ReferenceHealth,
    pub epoch: u64,
    pub last_sequence: Option<u64>,
    pub digest: [u8; 32],
    pub last_reference_received_ns: Option<i64>,
    pub symbols: BTreeMap<ReferenceSymbol, ReferenceSymbolSnapshot>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReferenceReplayState {
    health: ReferenceHealth,
    epoch: u64,
    last_sequence: Option<u64>,
    symbols: BTreeMap<ReferenceSymbol, SymbolState>,
    finalized: BTreeMap<(ReferenceSymbol, i64), FinalizedCandle>,
    last_reference_received_ns: Option<i64>,
}

impl Default for ReferenceReplayState {
    fn default() -> Self {
        Self {
            health: ReferenceHealth::Starting,
            epoch: 0,
            last_sequence: None,
            symbols: BTreeMap::new(),
            finalized: BTreeMap::new(),
            last_reference_received_ns: None,
        }
    }
}

impl ReferenceReplayState {
    #[must_use]
    pub const fn health(&self) -> ReferenceHealth {
        self.health
    }
    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }
    #[must_use]
    pub const fn last_sequence(&self) -> Option<u64> {
        self.last_sequence
    }
    #[must_use]
    pub fn finalized_candle(
        &self,
        symbol: ReferenceSymbol,
        open_time_ms: i64,
    ) -> Option<FinalizedCandle> {
        self.finalized.get(&(symbol, open_time_ms)).copied()
    }
    #[must_use]
    pub fn in_progress_candle(&self, symbol: ReferenceSymbol) -> Option<InProgressCandle> {
        self.symbols
            .get(&symbol)
            .and_then(|state| state.in_progress)
    }
    #[must_use]
    pub fn latest_book(&self, symbol: ReferenceSymbol) -> Option<BookTicker> {
        self.symbols
            .get(&symbol)
            .and_then(|state| state.book_ticker)
    }
    #[must_use]
    pub fn latest_trade(&self, symbol: ReferenceSymbol) -> Option<AggregateTrade> {
        self.symbols
            .get(&symbol)
            .and_then(|state| state.aggregate_trade)
    }

    #[must_use]
    pub fn snapshot(&self) -> ReferenceSnapshot {
        ReferenceSnapshot {
            health: self.health,
            epoch: self.epoch,
            last_sequence: self.last_sequence,
            digest: self.digest(),
            last_reference_received_ns: self.last_reference_received_ns,
            symbols: self
                .symbols
                .iter()
                .map(|(symbol, state)| {
                    (
                        *symbol,
                        ReferenceSymbolSnapshot {
                            candle_event_ns: state.candle_event_ns,
                            candle_received_ns: state.candle_received_ns,
                            aggregate_trade_event_ns: state.aggregate_trade_event_ns,
                            aggregate_trade_received_ns: state.aggregate_trade_received_ns,
                            book_ticker_received_ns: state.book_ticker_received_ns,
                        },
                    )
                })
                .collect(),
        }
    }

    /// Applies exactly one contiguous journal envelope.
    ///
    /// # Errors
    ///
    /// Fails closed on sequence, source, epoch, timestamp, or feed invariants.
    pub fn apply(&mut self, envelope: &EventEnvelope) -> Result<(), ReferenceReplayError> {
        let mut next = self.clone();
        next.apply_in_place(envelope)?;
        *self = next;
        Ok(())
    }

    fn apply_in_place(&mut self, envelope: &EventEnvelope) -> Result<(), ReferenceReplayError> {
        let expected = match self.last_sequence {
            Some(value) => value
                .checked_add(1)
                .ok_or(ReferenceReplayError::SequenceOverflow)?,
            None => envelope.sequence,
        };
        if envelope.sequence != expected {
            return Err(ReferenceReplayError::SequenceGap {
                expected,
                actual: envelope.sequence,
            });
        }
        match envelope.source {
            EventSource::System if envelope.market_id == SYSTEM_MARKET_ID => {
                self.apply_system(&envelope.payload)?;
            }
            EventSource::ReferencePrice => self.apply_reference(envelope)?,
            _ => return Err(ReferenceReplayError::UnexpectedSource(envelope.source)),
        }
        self.last_sequence = Some(envelope.sequence);
        Ok(())
    }

    fn apply_system(&mut self, payload: &[u8]) -> Result<(), ReferenceReplayError> {
        match payload {
            EPOCH_START => {
                self.epoch = self
                    .epoch
                    .checked_add(1)
                    .ok_or(ReferenceReplayError::SequenceOverflow)?;
                self.health = ReferenceHealth::Collecting;
                self.symbols.clear();
            }
            EPOCH_SYNCED if self.health == ReferenceHealth::Collecting => {
                if !self.all_predictive_sources_present() {
                    return Err(ReferenceReplayError::PrematureSync);
                }
                self.health = ReferenceHealth::Ready;
            }
            EPOCH_ROTATE | EPOCH_DISCONNECTED
                if matches!(
                    self.health,
                    ReferenceHealth::Collecting | ReferenceHealth::Ready
                ) =>
            {
                self.health = ReferenceHealth::Disconnected;
                self.symbols.clear();
            }
            EPOCH_SHUTDOWN
                if matches!(
                    self.health,
                    ReferenceHealth::Collecting | ReferenceHealth::Ready
                ) =>
            {
                self.health = ReferenceHealth::Shutdown;
                self.symbols.clear();
            }
            _ => return Err(ReferenceReplayError::InvalidSystemTransition),
        }
        Ok(())
    }

    fn apply_reference(&mut self, envelope: &EventEnvelope) -> Result<(), ReferenceReplayError> {
        if !matches!(
            self.health,
            ReferenceHealth::Collecting | ReferenceHealth::Ready
        ) {
            return Err(ReferenceReplayError::ReferenceOutsideEpoch);
        }
        let decoded = decode_reference_payload(&envelope.payload)?;
        let expected_market = format!("BINANCE_SPOT:{}", decoded.event.symbol().as_upper());
        if envelope.market_id != expected_market {
            return Err(ReferenceReplayError::MarketMismatch);
        }
        let expected_ns = match decoded.event_time_ms {
            Some(ms) => ms
                .checked_mul(1_000_000)
                .ok_or(ReferenceReplayError::TimestampOverflow)?,
            None => SOURCE_TIME_UNAVAILABLE_NS,
        };
        if envelope.event_time_ns != expected_ns {
            return Err(ReferenceReplayError::TimestampMismatch);
        }
        if envelope.received_time_ns < 0 {
            return Err(ReferenceReplayError::TimestampMismatch);
        }
        if self
            .last_reference_received_ns
            .is_some_and(|previous| envelope.received_time_ns < previous)
        {
            return Err(ReferenceReplayError::ReceiveTimeRegression);
        }

        let symbol = decoded.event.symbol();
        let state = self.symbols.entry(symbol).or_default();
        match decoded.event {
            ReferenceEvent::InProgressCandle(value) => {
                apply_in_progress(state, value)?;
                state.candle_event_ns = Some(envelope.event_time_ns);
                state.candle_received_ns = Some(envelope.received_time_ns);
            }
            ReferenceEvent::FinalizedCandle(value) => {
                apply_finalized(state, value)?;
                state.candle_event_ns = Some(envelope.event_time_ns);
                state.candle_received_ns = Some(envelope.received_time_ns);
                let key = (symbol, value.0.open_time_ms);
                if let Some(existing) = self.finalized.insert(key, value) {
                    if existing != value {
                        return Err(ReferenceReplayError::FinalizedCandleChanged);
                    }
                }
            }
            ReferenceEvent::AggregateTrade(value) => {
                if state
                    .aggregate_trade
                    .is_some_and(|prior| value.aggregate_trade_id <= prior.aggregate_trade_id)
                {
                    return Err(ReferenceReplayError::SourceSequenceRegression(
                        "aggregate trade",
                    ));
                }
                state.aggregate_trade = Some(value);
                state.aggregate_trade_event_ns = Some(envelope.event_time_ns);
                state.aggregate_trade_received_ns = Some(envelope.received_time_ns);
            }
            ReferenceEvent::BookTicker(value) => {
                if state
                    .book_ticker
                    .is_some_and(|prior| value.update_id <= prior.update_id)
                {
                    return Err(ReferenceReplayError::SourceSequenceRegression(
                        "book ticker",
                    ));
                }
                state.book_ticker = Some(value);
                state.book_ticker_received_ns = Some(envelope.received_time_ns);
            }
        }
        self.last_reference_received_ns = Some(envelope.received_time_ns);
        Ok(())
    }

    fn all_predictive_sources_present(&self) -> bool {
        ReferenceSymbol::ALL.iter().all(|symbol| {
            self.symbols.get(symbol).is_some_and(|state| {
                state.in_progress.is_some()
                    && state.aggregate_trade.is_some()
                    && state.book_ticker.is_some()
            })
        })
    }

    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"REFERENCE_REPLAY_STATE_V1");
        hasher.update(&self.epoch.to_le_bytes());
        hasher.update(&[self.health as u8]);
        hasher.update(&self.last_sequence.unwrap_or(u64::MAX).to_le_bytes());
        hasher.update(
            &self
                .last_reference_received_ns
                .unwrap_or(i64::MIN)
                .to_le_bytes(),
        );
        for ((symbol, open_time), candle) in &self.finalized {
            hasher.update(&[*symbol as u8]);
            hasher.update(&open_time.to_le_bytes());
            hash_candle(&mut hasher, candle.0);
        }
        for symbol in ReferenceSymbol::ALL {
            hasher.update(&[symbol as u8]);
            if let Some(state) = self.symbols.get(&symbol) {
                for timestamp in [
                    state.candle_event_ns,
                    state.candle_received_ns,
                    state.aggregate_trade_event_ns,
                    state.aggregate_trade_received_ns,
                    state.book_ticker_received_ns,
                ] {
                    hasher.update(&timestamp.unwrap_or(i64::MIN).to_le_bytes());
                }
                if let Some(candle) = state.in_progress {
                    hasher.update(&[1]);
                    hash_candle(&mut hasher, candle.0);
                } else {
                    hasher.update(&[0]);
                }
                if let Some(trade) = state.aggregate_trade {
                    hasher.update(&[1]);
                    hasher.update(&trade.aggregate_trade_id.to_le_bytes());
                    hasher.update(&trade.price.as_micros().to_le_bytes());
                    hasher.update(&trade.quantity.as_e8().to_le_bytes());
                } else {
                    hasher.update(&[0]);
                }
                if let Some(book) = state.book_ticker {
                    hasher.update(&[1]);
                    hasher.update(&book.update_id.to_le_bytes());
                    hasher.update(&book.best_bid.as_micros().to_le_bytes());
                    hasher.update(&book.best_ask.as_micros().to_le_bytes());
                } else {
                    hasher.update(&[0]);
                }
            } else {
                hasher.update(&[0, 0, 0]);
            }
        }
        *hasher.finalize().as_bytes()
    }
}

fn apply_in_progress(
    state: &mut SymbolState,
    value: InProgressCandle,
) -> Result<(), ReferenceReplayError> {
    if state
        .last_finalized
        .is_some_and(|last| value.0.open_time_ms <= last.0.open_time_ms)
    {
        return Err(ReferenceReplayError::CandleRegression);
    }
    if state
        .in_progress
        .is_some_and(|prior| value.0.open_time_ms < prior.0.open_time_ms)
    {
        return Err(ReferenceReplayError::CandleRegression);
    }
    state.in_progress = Some(value);
    Ok(())
}
fn apply_finalized(
    state: &mut SymbolState,
    value: FinalizedCandle,
) -> Result<(), ReferenceReplayError> {
    if state
        .last_finalized
        .is_some_and(|last| value.0.open_time_ms <= last.0.open_time_ms)
    {
        return Err(ReferenceReplayError::CandleRegression);
    }
    if state
        .in_progress
        .is_some_and(|progress| progress.0.open_time_ms > value.0.open_time_ms)
    {
        return Err(ReferenceReplayError::CandleRegression);
    }
    state.last_finalized = Some(value);
    if state
        .in_progress
        .is_some_and(|progress| progress.0.open_time_ms == value.0.open_time_ms)
    {
        state.in_progress = None;
    }
    Ok(())
}
fn hash_candle(hasher: &mut blake3::Hasher, value: CandleData) {
    for item in [
        value.open_time_ms,
        value.close_time_ms,
        value.first_trade_id,
        value.last_trade_id,
        value.open.as_micros(),
        value.high.as_micros(),
        value.low.as_micros(),
        value.close.as_micros(),
        value.base_volume.as_e8(),
        value.quote_volume.as_e8(),
    ] {
        hasher.update(&item.to_le_bytes());
    }
    hasher.update(&value.trade_count.to_le_bytes());
}

#[derive(Debug, Error)]
pub enum ReferenceReplayError {
    #[error("journal error: {0}")]
    Journal(#[from] JournalError),
    #[error("reference payload error: {0}")]
    Payload(#[from] PayloadError),
    #[error("journal is not cleanly terminated")]
    IncompleteJournal,
    #[error("sequence gap: expected {expected}, got {actual}")]
    SequenceGap { expected: u64, actual: u64 },
    #[error("sequence or epoch overflow")]
    SequenceOverflow,
    #[error("unexpected event source: {0:?}")]
    UnexpectedSource(EventSource),
    #[error("invalid reference gateway system transition")]
    InvalidSystemTransition,
    #[error("gateway declared sync before all required feeds were observed")]
    PrematureSync,
    #[error("reference event arrived outside an active epoch")]
    ReferenceOutsideEpoch,
    #[error("reference market identifier disagrees with payload")]
    MarketMismatch,
    #[error("envelope and payload timestamps disagree")]
    TimestampMismatch,
    #[error("reference receive time regressed")]
    ReceiveTimeRegression,
    #[error("timestamp overflow")]
    TimestampOverflow,
    #[error("source sequence regressed for {0}")]
    SourceSequenceRegression(&'static str),
    #[error("candle time regressed")]
    CandleRegression,
    #[error("a finalized candle changed")]
    FinalizedCandleChanged,
}

/// Streams a clean single journal into deterministic reference-feed state.
///
/// # Errors
///
/// Returns [`ReferenceReplayError`] for journal, sequence, payload, timestamp,
/// epoch, or source-state invariant failures.
pub fn replay_path(path: impl AsRef<Path>) -> Result<ReferenceReplayState, ReferenceReplayError> {
    let mut reader = JournalReader::open(path)?;
    let mut state = ReferenceReplayState::default();
    while let Some(event) = reader.next_event()? {
        state.apply(&event)?;
    }
    if reader.tail() != Some(JournalTail::Clean) {
        return Err(ReferenceReplayError::IncompleteJournal);
    }
    Ok(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::payload::{encode_reference_payload, parse_combined_message};
    use market_recorder::JournalWriter;
    use tempfile::tempdir;

    fn system(sequence: u64, payload: &[u8]) -> EventEnvelope {
        EventEnvelope::new(
            EventSource::System,
            sequence,
            1,
            1,
            SYSTEM_MARKET_ID.to_owned(),
            payload.to_vec(),
        )
        .expect("system envelope")
    }

    fn reference(sequence: u64, json: &str) -> EventEnvelope {
        let decoded = parse_combined_message(json).expect("parse");
        let event_time_ns = decoded
            .event_time_ms
            .map_or(SOURCE_TIME_UNAVAILABLE_NS, |value| value * 1_000_000);
        EventEnvelope::new(
            EventSource::ReferencePrice,
            sequence,
            event_time_ns,
            2,
            format!("BINANCE_SPOT:{}", decoded.event.symbol().as_upper()),
            encode_reference_payload(decoded).expect("payload"),
        )
        .expect("reference envelope")
    }

    fn sample(symbol: &str, stream_symbol: &str, kind: u8, id: u64) -> String {
        match kind {
            1 => format!(
                r#"{{"stream":"{stream_symbol}@kline_1h","data":{{"e":"kline","E":1000,"s":"{symbol}","k":{{"t":0,"T":3599999,"s":"{symbol}","i":"1h","f":1,"L":2,"o":"100","c":"101","h":"102","l":"99","v":"1","n":2,"x":false,"q":"100"}}}}}}"#
            ),
            2 => format!(
                r#"{{"stream":"{stream_symbol}@aggTrade","data":{{"e":"aggTrade","E":1000,"s":"{symbol}","a":{id},"p":"101","q":"1","f":1,"l":2,"T":1000,"m":false}}}}"#
            ),
            _ => format!(
                r#"{{"stream":"{stream_symbol}@bookTicker","data":{{"u":{id},"s":"{symbol}","b":"100","B":"1","a":"101","A":"1"}}}}"#
            ),
        }
    }

    #[test]
    fn epoch_becomes_ready_only_after_all_independent_feeds() {
        let mut state = ReferenceReplayState::default();
        state.apply(&system(0, EPOCH_START)).expect("start");
        let mut sequence = 1;
        for (symbol, stream) in [("BTCUSDT", "btcusdt"), ("ETHUSDT", "ethusdt")] {
            for kind in 1..=3 {
                state
                    .apply(&reference(sequence, &sample(symbol, stream, kind, 10)))
                    .expect("feed");
                sequence += 1;
            }
        }
        assert_eq!(state.health(), ReferenceHealth::Collecting);
        state.apply(&system(sequence, EPOCH_SYNCED)).expect("sync");
        assert_eq!(state.health(), ReferenceHealth::Ready);
        assert_eq!(state.digest(), state.clone().digest());
    }

    #[test]
    fn regression_and_sequence_failures_are_transactional() {
        let mut state = ReferenceReplayState::default();
        state.apply(&system(7, EPOCH_START)).expect("start");
        state
            .apply(&reference(8, &sample("BTCUSDT", "btcusdt", 2, 10)))
            .expect("trade");
        let before = state.clone();
        assert!(matches!(
            state.apply(&reference(9, &sample("BTCUSDT", "btcusdt", 2, 9))),
            Err(ReferenceReplayError::SourceSequenceRegression(_))
        ));
        assert_eq!(state, before);
        assert!(matches!(
            state.apply(&reference(10, &sample("BTCUSDT", "btcusdt", 2, 11))),
            Err(ReferenceReplayError::SequenceGap { .. })
        ));
        assert_eq!(state, before);
    }

    #[test]
    fn receive_time_regression_is_transactional() {
        let mut state = ReferenceReplayState::default();
        state.apply(&system(0, EPOCH_START)).expect("start");
        let first = reference(1, &sample("BTCUSDT", "btcusdt", 2, 10));
        state.apply(&first).expect("first trade");
        let before = state.clone();
        let mut regressed = reference(2, &sample("BTCUSDT", "btcusdt", 2, 11));
        regressed.received_time_ns = first.received_time_ns - 1;
        assert!(matches!(
            state.apply(&regressed),
            Err(ReferenceReplayError::ReceiveTimeRegression)
        ));
        assert_eq!(state, before);
    }

    #[test]
    fn checksummed_journal_replay_matches_live_application() {
        let directory = tempdir().expect("directory");
        let path = directory.path().join("reference.journal");
        let events = [
            system(0, EPOCH_START),
            reference(1, &sample("BTCUSDT", "btcusdt", 1, 1)),
            reference(2, &sample("BTCUSDT", "btcusdt", 2, 2)),
            reference(3, &sample("BTCUSDT", "btcusdt", 3, 3)),
            reference(4, &sample("ETHUSDT", "ethusdt", 1, 1)),
            reference(5, &sample("ETHUSDT", "ethusdt", 2, 2)),
            reference(6, &sample("ETHUSDT", "ethusdt", 3, 3)),
            system(7, EPOCH_SYNCED),
            system(8, EPOCH_SHUTDOWN),
        ];
        let mut live = ReferenceReplayState::default();
        let mut writer = JournalWriter::open(&path).expect("journal");
        for event in &events {
            live.apply(event).expect("live apply");
            writer.append(event).expect("append");
        }
        writer.sync().expect("sync");
        drop(writer);
        let replayed = replay_path(path).expect("replay");
        assert_eq!(replayed, live);
        assert_eq!(replayed.digest(), live.digest());
        assert_eq!(replayed.health(), ReferenceHealth::Shutdown);
    }
}
