#![forbid(unsafe_code)]

//! Deterministic, read-only reconstruction of public token order books.

use common_types::{PriceMicros, QuantityMicros};
use event_schema::{EventEnvelope, EventSource};
use market_recorder::{
    JournalError, JournalReader, JournalTail, SegmentError, SegmentedJournalReader,
};
use public_market_data::{
    decode_public_payload, BestBidAsk, BookSnapshot, LastTrade, MarketSide, PayloadError,
    PriceChange, PublicMarketEvent, TickSizeChange,
};
use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

const EPOCH_START: &[u8] = b"PUBLIC_MARKET_EPOCH_START_V1";
const EPOCH_SYNCED: &[u8] = b"PUBLIC_MARKET_EPOCH_SYNCED_V1";
const EPOCH_REDISCOVER: &[u8] = b"PUBLIC_MARKET_EPOCH_REDISCOVER_V1";
const EPOCH_SHUTDOWN: &[u8] = b"PUBLIC_MARKET_EPOCH_SHUTDOWN_V1";
const SYSTEM_MARKET_ID: &str = "__public_market_gateway__";
const CHECKPOINT_MAGIC: &[u8; 8] = b"POLYCHK1";
const CHECKPOINT_VERSION: u16 = 1;
const CHECKPOINT_HEADER_BYTES: usize = 26;
const CHECKPOINT_CHECKSUM_BYTES: usize = 32;
const MAX_CHECKPOINT_BYTES: usize = 256 * 1024 * 1024;
const MAX_CHECKPOINT_BOOKS: usize = 4_096;
const MAX_CHECKPOINT_LEVELS: usize = 1_000_000;
const MAX_CHECKPOINT_STRING: usize = 4 * 1024;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[repr(u8)]
pub enum EpochStatus {
    Inactive = 0,
    CollectingSnapshots = 1,
    Synchronized = 2,
    Shutdown = 3,
}

#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct BookKey {
    pub condition_id: String,
    pub asset_id: String,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct TokenBook {
    bids: BTreeMap<PriceMicros, QuantityMicros>,
    asks: BTreeMap<PriceMicros, QuantityMicros>,
    tick_size: Option<PriceMicros>,
    last_trade: Option<(MarketSide, PriceMicros, Option<QuantityMicros>)>,
    authoritative: bool,
}

impl TokenBook {
    #[must_use]
    pub fn best_bid(&self) -> Option<(PriceMicros, QuantityMicros)> {
        self.bids
            .iter()
            .next_back()
            .map(|(price, size)| (*price, *size))
    }

    #[must_use]
    pub fn best_ask(&self) -> Option<(PriceMicros, QuantityMicros)> {
        self.asks.iter().next().map(|(price, size)| (*price, *size))
    }

    #[must_use]
    pub fn bid_levels(&self) -> usize {
        self.bids.len()
    }

    #[must_use]
    pub fn ask_levels(&self) -> usize {
        self.asks.len()
    }

    #[must_use]
    pub const fn is_authoritative(&self) -> bool {
        self.authoritative
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ReplayState {
    epoch: u64,
    status: EpochStatus,
    last_sequence: Option<u64>,
    books: BTreeMap<BookKey, TokenBook>,
}

impl Default for ReplayState {
    fn default() -> Self {
        Self {
            epoch: 0,
            status: EpochStatus::Inactive,
            last_sequence: None,
            books: BTreeMap::new(),
        }
    }
}

impl ReplayState {
    #[must_use]
    pub const fn epoch(&self) -> u64 {
        self.epoch
    }

    #[must_use]
    pub const fn status(&self) -> EpochStatus {
        self.status
    }

    #[must_use]
    pub const fn last_sequence(&self) -> Option<u64> {
        self.last_sequence
    }

    #[must_use]
    pub fn books(&self) -> &BTreeMap<BookKey, TokenBook> {
        &self.books
    }

    #[must_use]
    pub fn is_authoritative(&self) -> bool {
        self.status == EpochStatus::Synchronized
            && !self.books.is_empty()
            && self.books.values().all(TokenBook::is_authoritative)
    }

    /// Applies one envelope atomically to this single-writer state.
    ///
    /// # Errors
    ///
    /// Returns [`ReplayError`] on sequence gaps, invalid epochs, malformed
    /// payloads, identity mismatches, or book invariant violations. State is
    /// unchanged on error.
    pub fn apply(&mut self, envelope: &EventEnvelope) -> Result<(), ReplayError> {
        validate_sequence(self.last_sequence, envelope.sequence)?;
        let mut candidate = self.clone();
        candidate.apply_validated(envelope)?;
        candidate.last_sequence = Some(envelope.sequence);
        *self = candidate;
        Ok(())
    }

    fn apply_validated(&mut self, envelope: &EventEnvelope) -> Result<(), ReplayError> {
        match envelope.source {
            EventSource::System => self.apply_system(envelope),
            EventSource::Market => self.apply_market(envelope),
            source => Err(ReplayError::UnsupportedSource(source)),
        }
    }

    fn apply_system(&mut self, envelope: &EventEnvelope) -> Result<(), ReplayError> {
        if envelope.market_id != SYSTEM_MARKET_ID {
            return Err(ReplayError::SystemIdentity);
        }
        match envelope.payload.as_slice() {
            EPOCH_START => {
                self.epoch = self
                    .epoch
                    .checked_add(1)
                    .ok_or(ReplayError::EpochOverflow)?;
                self.status = EpochStatus::CollectingSnapshots;
                self.books.clear();
                Ok(())
            }
            EPOCH_SYNCED => {
                if self.status != EpochStatus::CollectingSnapshots || self.books.is_empty() {
                    return Err(ReplayError::InvalidEpochTransition);
                }
                self.status = EpochStatus::Synchronized;
                Ok(())
            }
            EPOCH_REDISCOVER => {
                if !matches!(
                    self.status,
                    EpochStatus::CollectingSnapshots | EpochStatus::Synchronized
                ) {
                    return Err(ReplayError::InvalidEpochTransition);
                }
                self.status = EpochStatus::Inactive;
                self.books.clear();
                Ok(())
            }
            EPOCH_SHUTDOWN => {
                if !matches!(
                    self.status,
                    EpochStatus::CollectingSnapshots | EpochStatus::Synchronized
                ) {
                    return Err(ReplayError::InvalidEpochTransition);
                }
                self.status = EpochStatus::Shutdown;
                Ok(())
            }
            _ => Err(ReplayError::UnknownSystemEvent),
        }
    }

    fn apply_market(&mut self, envelope: &EventEnvelope) -> Result<(), ReplayError> {
        if !matches!(
            self.status,
            EpochStatus::CollectingSnapshots | EpochStatus::Synchronized
        ) {
            return Err(ReplayError::MarketOutsideEpoch);
        }
        let decoded = decode_public_payload(&envelope.payload)?;
        let expected_event_time = decoded
            .timestamp_ms
            .checked_mul(1_000_000)
            .ok_or(ReplayError::TimestampOverflow)?;
        if expected_event_time != envelope.event_time_ns {
            return Err(ReplayError::EnvelopeMismatch("event timestamp"));
        }
        let condition = event_condition(&decoded.event);
        if condition != envelope.market_id {
            return Err(ReplayError::EnvelopeMismatch("condition ID"));
        }

        match decoded.event {
            PublicMarketEvent::Book(snapshot) => self.apply_snapshot(snapshot),
            PublicMarketEvent::PriceChanges {
                condition_id,
                changes,
            } => self.apply_price_changes(&condition_id, &changes),
            PublicMarketEvent::TickSizeChange(change) => self.apply_tick_size(&change),
            PublicMarketEvent::LastTrade(trade) => self.apply_last_trade(&trade),
            PublicMarketEvent::BestBidAsk(update) => self.apply_best_bid_ask(&update),
        }
    }

    fn apply_snapshot(&mut self, snapshot: BookSnapshot) -> Result<(), ReplayError> {
        let bids = levels_to_map(&snapshot.bids)?;
        let asks = levels_to_map(&snapshot.asks)?;
        validate_not_crossed(&bids, &asks)?;
        self.books.insert(
            BookKey {
                condition_id: snapshot.condition_id,
                asset_id: snapshot.asset_id,
            },
            TokenBook {
                bids,
                asks,
                tick_size: None,
                last_trade: None,
                authoritative: true,
            },
        );
        Ok(())
    }

    fn apply_price_changes(
        &mut self,
        condition_id: &str,
        changes: &[PriceChange],
    ) -> Result<(), ReplayError> {
        let mut affected = BTreeSet::new();
        for change in changes {
            let key = BookKey {
                condition_id: condition_id.to_owned(),
                asset_id: change.asset_id.clone(),
            };
            if !self.books.contains_key(&key) {
                return Err(ReplayError::DeltaBeforeSnapshot(key));
            }
            affected.insert(key);
        }
        for change in changes {
            let key = BookKey {
                condition_id: condition_id.to_owned(),
                asset_id: change.asset_id.clone(),
            };
            let book = self
                .books
                .get_mut(&key)
                .ok_or_else(|| ReplayError::DeltaBeforeSnapshot(key.clone()))?;
            if !book.authoritative {
                continue;
            }
            let side = match change.side {
                MarketSide::Bid => &mut book.bids,
                MarketSide::Ask => &mut book.asks,
            };
            if change.quantity == QuantityMicros::ZERO {
                side.remove(&change.price);
            } else {
                side.insert(change.price, change.quantity);
            }
        }
        for key in affected {
            let book = self
                .books
                .get_mut(&key)
                .ok_or_else(|| ReplayError::DeltaBeforeSnapshot(key.clone()))?;
            if validate_not_crossed(&book.bids, &book.asks).is_err() {
                book.authoritative = false;
            }
        }
        Ok(())
    }

    fn apply_tick_size(&mut self, change: &TickSizeChange) -> Result<(), ReplayError> {
        let key = BookKey {
            condition_id: change.condition_id.clone(),
            asset_id: change.asset_id.clone(),
        };
        let book = self
            .books
            .get_mut(&key)
            .ok_or_else(|| ReplayError::DeltaBeforeSnapshot(key.clone()))?;
        if book
            .tick_size
            .is_some_and(|current| current != change.old_tick_size)
        {
            return Err(ReplayError::TickSizeMismatch);
        }
        book.tick_size = Some(change.new_tick_size);
        Ok(())
    }

    fn apply_last_trade(&mut self, trade: &LastTrade) -> Result<(), ReplayError> {
        let key = BookKey {
            condition_id: trade.condition_id.clone(),
            asset_id: trade.asset_id.clone(),
        };
        let book = self
            .books
            .get_mut(&key)
            .ok_or_else(|| ReplayError::DeltaBeforeSnapshot(key.clone()))?;
        book.last_trade = Some((trade.side, trade.price, trade.quantity));
        Ok(())
    }

    fn apply_best_bid_ask(&mut self, update: &BestBidAsk) -> Result<(), ReplayError> {
        let key = BookKey {
            condition_id: update.condition_id.clone(),
            asset_id: update.asset_id.clone(),
        };
        let book = self
            .books
            .get_mut(&key)
            .ok_or_else(|| ReplayError::DeltaBeforeSnapshot(key.clone()))?;
        if !book.authoritative {
            return Ok(());
        }
        let best_bid = book.best_bid().map(|(price, _)| price);
        let best_ask = book.best_ask().map(|(price, _)| price);
        if best_bid != Some(update.best_bid) || best_ask != Some(update.best_ask) {
            book.authoritative = false;
        }
        Ok(())
    }

    /// Computes a stable digest from explicitly encoded canonical state.
    #[must_use]
    pub fn digest(&self) -> [u8; 32] {
        let mut hasher = blake3::Hasher::new();
        hasher.update(b"POLY_BOOK_STATE_V1");
        hasher.update(&self.epoch.to_le_bytes());
        hasher.update(&[self.status as u8]);
        encode_option_u64(&mut hasher, self.last_sequence);
        encode_length(&mut hasher, self.books.len());
        for (key, book) in &self.books {
            encode_string(&mut hasher, &key.condition_id);
            encode_string(&mut hasher, &key.asset_id);
            encode_levels(&mut hasher, &book.bids);
            encode_levels(&mut hasher, &book.asks);
            hasher.update(&[u8::from(book.authoritative)]);
            encode_option_price(&mut hasher, book.tick_size);
            match book.last_trade {
                Some((side, price, quantity)) => {
                    hasher.update(&[1, side_byte(side)]);
                    hasher.update(&price.as_micros().to_le_bytes());
                    match quantity {
                        Some(quantity) => {
                            hasher.update(&[1]);
                            hasher.update(&quantity.as_micros().to_le_bytes());
                        }
                        None => {
                            hasher.update(&[0]);
                        }
                    }
                }
                None => {
                    hasher.update(&[0]);
                }
            }
        }
        *hasher.finalize().as_bytes()
    }
}

#[derive(Debug, Error)]
pub enum ReplayError {
    #[error("journal error: {0}")]
    Journal(#[from] JournalError),
    #[error("segmented journal error: {0}")]
    Segment(#[from] SegmentError),
    #[error("journal has an incomplete tail")]
    IncompleteJournal,
    #[error("unsupported event source: {0:?}")]
    UnsupportedSource(EventSource),
    #[error("sequence duplicate or regression: expected {expected}, received {actual}")]
    SequenceRegression { expected: u64, actual: u64 },
    #[error("sequence gap: expected {expected}, received {actual}")]
    SequenceGap { expected: u64, actual: u64 },
    #[error("sequence exhausted")]
    SequenceExhausted,
    #[error("epoch counter overflow")]
    EpochOverflow,
    #[error("invalid system event identity")]
    SystemIdentity,
    #[error("unknown public recorder system event")]
    UnknownSystemEvent,
    #[error("invalid epoch transition")]
    InvalidEpochTransition,
    #[error("market event occurred outside an active epoch")]
    MarketOutsideEpoch,
    #[error("public payload error: {0}")]
    Payload(#[from] PayloadError),
    #[error("public event timestamp overflow")]
    TimestampOverflow,
    #[error("envelope and payload disagree: {0}")]
    EnvelopeMismatch(&'static str),
    #[error("duplicate price level")]
    DuplicateLevel,
    #[error("snapshot contains a zero-sized level")]
    ZeroSnapshotLevel,
    #[error("order book is crossed")]
    CrossedBook,
    #[error("delta arrived before a fresh snapshot for {0:?}")]
    DeltaBeforeSnapshot(BookKey),
    #[error("tick-size transition does not match current state")]
    TickSizeMismatch,
    #[error("reported best bid/ask does not match reconstructed state")]
    BestPriceMismatch,
    #[error("checkpoint does not match its durable journal prefix")]
    CheckpointMismatch,
    #[error("checkpoint sequence is not present in the segmented journal")]
    CheckpointSequenceNotFound,
}

/// Replays a clean journal into deterministic order-book state.
///
/// # Errors
///
/// Returns [`ReplayError`] for journal corruption/tails or any state-machine
/// invariant failure.
pub fn replay_path(path: impl AsRef<Path>) -> Result<ReplayState, ReplayError> {
    let mut reader = JournalReader::open(path)?;
    let mut state = ReplayState::default();
    while let Some(event) = reader.next_event()? {
        state.apply(&event)?;
    }
    if reader.tail() != Some(JournalTail::Clean) {
        return Err(ReplayError::IncompleteJournal);
    }
    Ok(state)
}

/// Streams contiguous segments into deterministic order-book state.
///
/// # Errors
///
/// Returns [`ReplayError`] for directory, segment, sequence, payload, or state
/// invariant failures.
pub fn replay_segmented_path(path: impl AsRef<Path>) -> Result<ReplayState, ReplayError> {
    let mut reader = SegmentedJournalReader::open(path)?;
    let mut state = ReplayState::default();
    while let Some(event) = reader.next_event()? {
        state.apply(&event)?;
    }
    Ok(state)
}

/// Validates a checkpoint against its durable prefix, then applies later
/// segmented events.
///
/// # Errors
///
/// Returns [`ReplayError::CheckpointMismatch`] unless the checkpoint digest and
/// sequence exactly match replay of the durable prefix.
pub fn replay_segmented_from_checkpoint(
    path: impl AsRef<Path>,
    checkpoint: &ReplayState,
) -> Result<ReplayState, ReplayError> {
    let target = checkpoint.last_sequence();
    let mut reader = SegmentedJournalReader::open(path)?;
    let mut state = ReplayState::default();
    let mut verified = target.is_none() && state.digest() == checkpoint.digest();
    if target.is_none() && !verified {
        return Err(ReplayError::CheckpointMismatch);
    }
    while let Some(event) = reader.next_event()? {
        if verified {
            state.apply(&event)?;
            continue;
        }
        state.apply(&event)?;
        if Some(event.sequence) == target {
            if state.digest() != checkpoint.digest() {
                return Err(ReplayError::CheckpointMismatch);
            }
            state = checkpoint.clone();
            verified = true;
        } else if target.is_some_and(|target| event.sequence > target) {
            return Err(ReplayError::CheckpointSequenceNotFound);
        }
    }
    if !verified {
        return Err(ReplayError::CheckpointSequenceNotFound);
    }
    Ok(state)
}

#[derive(Debug, Error)]
pub enum CheckpointError {
    #[error("checkpoint I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("checkpoint is truncated")]
    Truncated,
    #[error("checkpoint magic is invalid")]
    InvalidMagic,
    #[error("unsupported checkpoint version: {0}")]
    UnsupportedVersion(u16),
    #[error("checkpoint reserved bytes are non-zero")]
    ReservedBytes,
    #[error("checkpoint length is invalid or exceeds the bound")]
    InvalidLength,
    #[error("checkpoint checksum mismatch")]
    ChecksumMismatch,
    #[error("checkpoint contains invalid UTF-8")]
    InvalidUtf8,
    #[error("checkpoint contains an invalid epoch status: {0}")]
    InvalidStatus(u8),
    #[error("checkpoint contains an invalid market side: {0}")]
    InvalidSide(u8),
    #[error("checkpoint boolean is not canonical: {0}")]
    InvalidBoolean(u8),
    #[error("checkpoint exceeds a collection bound")]
    CollectionBound,
    #[error("checkpoint contains a duplicate key or price")]
    Duplicate,
    #[error("checkpoint contains an invalid financial value")]
    InvalidFinancialValue,
    #[error("checkpoint contains a zero snapshot level")]
    ZeroLevel,
    #[error("checkpoint contains a crossed authoritative book")]
    CrossedBook,
    #[error("checkpoint has trailing bytes")]
    TrailingBytes,
}

impl ReplayState {
    /// Encodes complete replay state using an explicit checksummed wire format.
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError`] when a length or configured collection bound
    /// cannot be represented.
    pub fn encode_checkpoint(&self) -> Result<Vec<u8>, CheckpointError> {
        if self.books.len() > MAX_CHECKPOINT_BOOKS {
            return Err(CheckpointError::CollectionBound);
        }
        let mut payload = Vec::new();
        payload.extend_from_slice(&self.epoch.to_le_bytes());
        payload.push(self.status as u8);
        encode_optional_u64(&mut payload, self.last_sequence);
        encode_u32_length(&mut payload, self.books.len())?;
        for (key, book) in &self.books {
            encode_checkpoint_string(&mut payload, &key.condition_id)?;
            encode_checkpoint_string(&mut payload, &key.asset_id)?;
            payload.push(u8::from(book.authoritative));
            encode_checkpoint_levels(&mut payload, &book.bids)?;
            encode_checkpoint_levels(&mut payload, &book.asks)?;
            encode_optional_price_bytes(&mut payload, book.tick_size);
            match book.last_trade {
                Some((side, price, quantity)) => {
                    payload.push(1);
                    payload.push(side_byte(side));
                    payload.extend_from_slice(&price.as_micros().to_le_bytes());
                    match quantity {
                        Some(quantity) => {
                            payload.push(1);
                            payload.extend_from_slice(&quantity.as_micros().to_le_bytes());
                        }
                        None => payload.push(0),
                    }
                }
                None => payload.push(0),
            }
        }
        if payload.len() > MAX_CHECKPOINT_BYTES {
            return Err(CheckpointError::InvalidLength);
        }
        let payload_length =
            u64::try_from(payload.len()).map_err(|_| CheckpointError::InvalidLength)?;
        let capacity = CHECKPOINT_HEADER_BYTES
            .checked_add(payload.len())
            .and_then(|value| value.checked_add(CHECKPOINT_CHECKSUM_BYTES))
            .ok_or(CheckpointError::InvalidLength)?;
        let mut output = Vec::with_capacity(capacity);
        output.extend_from_slice(CHECKPOINT_MAGIC);
        output.extend_from_slice(&CHECKPOINT_VERSION.to_le_bytes());
        output.extend_from_slice(&[0; 8]);
        output.extend_from_slice(&payload_length.to_le_bytes());
        output.extend_from_slice(&payload);
        output.extend_from_slice(blake3::hash(&payload).as_bytes());
        Ok(output)
    }

    /// Decodes and validates complete replay state from checkpoint bytes.
    ///
    /// # Errors
    ///
    /// Returns [`CheckpointError`] for checksum, schema, bounds, financial,
    /// canonical ordering, or structural failures.
    pub fn decode_checkpoint(bytes: &[u8]) -> Result<Self, CheckpointError> {
        if bytes.len() < CHECKPOINT_HEADER_BYTES + CHECKPOINT_CHECKSUM_BYTES {
            return Err(CheckpointError::Truncated);
        }
        if bytes.get(..8) != Some(CHECKPOINT_MAGIC.as_slice()) {
            return Err(CheckpointError::InvalidMagic);
        }
        let version = u16::from_le_bytes([bytes[8], bytes[9]]);
        if version != CHECKPOINT_VERSION {
            return Err(CheckpointError::UnsupportedVersion(version));
        }
        if bytes[10..18].iter().any(|byte| *byte != 0) {
            return Err(CheckpointError::ReservedBytes);
        }
        let length_start = 18;
        let length_end = CHECKPOINT_HEADER_BYTES;
        let payload_length = usize::try_from(u64::from_le_bytes(
            bytes[length_start..length_end]
                .try_into()
                .map_err(|_| CheckpointError::Truncated)?,
        ))
        .map_err(|_| CheckpointError::InvalidLength)?;
        if payload_length > MAX_CHECKPOINT_BYTES {
            return Err(CheckpointError::InvalidLength);
        }
        let payload_start = length_end;
        let payload_end = payload_start
            .checked_add(payload_length)
            .ok_or(CheckpointError::InvalidLength)?;
        let expected_end = payload_end
            .checked_add(CHECKPOINT_CHECKSUM_BYTES)
            .ok_or(CheckpointError::InvalidLength)?;
        if bytes.len() < expected_end {
            return Err(CheckpointError::Truncated);
        }
        if bytes.len() != expected_end {
            return Err(CheckpointError::TrailingBytes);
        }
        let payload = &bytes[payload_start..payload_end];
        if blake3::hash(payload).as_bytes() != &bytes[payload_end..expected_end] {
            return Err(CheckpointError::ChecksumMismatch);
        }
        decode_checkpoint_payload(payload)
    }
}

/// Writes a new checkpoint and refuses to replace an existing file.
///
/// # Errors
///
/// Returns [`CheckpointError`] for encoding, creation, write, or sync failures.
pub fn write_checkpoint(
    path: impl AsRef<Path>,
    state: &ReplayState,
) -> Result<(), CheckpointError> {
    let encoded = state.encode_checkpoint()?;
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(&encoded)?;
    file.sync_all()?;
    Ok(())
}

/// Reads a bounded checkpoint file and validates its checksum and schema.
///
/// # Errors
///
/// Returns [`CheckpointError`] for I/O, size, checksum, or decoding failures.
pub fn read_checkpoint(path: impl AsRef<Path>) -> Result<ReplayState, CheckpointError> {
    let path = path.as_ref();
    let length = fs::metadata(path)?.len();
    let maximum = CHECKPOINT_HEADER_BYTES
        .checked_add(MAX_CHECKPOINT_BYTES)
        .and_then(|value| value.checked_add(CHECKPOINT_CHECKSUM_BYTES))
        .ok_or(CheckpointError::InvalidLength)?;
    if length > u64::try_from(maximum).map_err(|_| CheckpointError::InvalidLength)? {
        return Err(CheckpointError::InvalidLength);
    }
    ReplayState::decode_checkpoint(&fs::read(path)?)
}

fn validate_sequence(last: Option<u64>, actual: u64) -> Result<(), ReplayError> {
    let Some(last) = last else {
        return Ok(());
    };
    let expected = last.checked_add(1).ok_or(ReplayError::SequenceExhausted)?;
    match actual.cmp(&expected) {
        std::cmp::Ordering::Less => Err(ReplayError::SequenceRegression { expected, actual }),
        std::cmp::Ordering::Greater => Err(ReplayError::SequenceGap { expected, actual }),
        std::cmp::Ordering::Equal => Ok(()),
    }
}

fn event_condition(event: &PublicMarketEvent) -> &str {
    match event {
        PublicMarketEvent::Book(event) => &event.condition_id,
        PublicMarketEvent::PriceChanges { condition_id, .. } => condition_id,
        PublicMarketEvent::TickSizeChange(event) => &event.condition_id,
        PublicMarketEvent::LastTrade(event) => &event.condition_id,
        PublicMarketEvent::BestBidAsk(event) => &event.condition_id,
    }
}

fn levels_to_map(
    levels: &[public_market_data::BookLevel],
) -> Result<BTreeMap<PriceMicros, QuantityMicros>, ReplayError> {
    let mut result = BTreeMap::new();
    for level in levels {
        if level.quantity == QuantityMicros::ZERO {
            return Err(ReplayError::ZeroSnapshotLevel);
        }
        if result.insert(level.price, level.quantity).is_some() {
            return Err(ReplayError::DuplicateLevel);
        }
    }
    Ok(result)
}

fn validate_not_crossed(
    bids: &BTreeMap<PriceMicros, QuantityMicros>,
    asks: &BTreeMap<PriceMicros, QuantityMicros>,
) -> Result<(), ReplayError> {
    if bids
        .keys()
        .next_back()
        .zip(asks.keys().next())
        .is_some_and(|(bid, ask)| bid >= ask)
    {
        Err(ReplayError::CrossedBook)
    } else {
        Ok(())
    }
}

fn encode_length(hasher: &mut blake3::Hasher, length: usize) {
    hasher.update(&u64::try_from(length).unwrap_or(u64::MAX).to_le_bytes());
}

fn encode_string(hasher: &mut blake3::Hasher, value: &str) {
    encode_length(hasher, value.len());
    hasher.update(value.as_bytes());
}

fn encode_levels(hasher: &mut blake3::Hasher, levels: &BTreeMap<PriceMicros, QuantityMicros>) {
    encode_length(hasher, levels.len());
    for (price, quantity) in levels {
        hasher.update(&price.as_micros().to_le_bytes());
        hasher.update(&quantity.as_micros().to_le_bytes());
    }
}

fn encode_option_u64(hasher: &mut blake3::Hasher, value: Option<u64>) {
    match value {
        Some(value) => {
            hasher.update(&[1]);
            hasher.update(&value.to_le_bytes());
        }
        None => {
            hasher.update(&[0]);
        }
    }
}

fn encode_option_price(hasher: &mut blake3::Hasher, value: Option<PriceMicros>) {
    match value {
        Some(value) => {
            hasher.update(&[1]);
            hasher.update(&value.as_micros().to_le_bytes());
        }
        None => {
            hasher.update(&[0]);
        }
    }
}

const fn side_byte(side: MarketSide) -> u8 {
    match side {
        MarketSide::Bid => 1,
        MarketSide::Ask => 2,
    }
}

fn encode_optional_u64(output: &mut Vec<u8>, value: Option<u64>) {
    match value {
        Some(value) => {
            output.push(1);
            output.extend_from_slice(&value.to_le_bytes());
        }
        None => output.push(0),
    }
}

fn encode_u32_length(output: &mut Vec<u8>, length: usize) -> Result<(), CheckpointError> {
    let length = u32::try_from(length).map_err(|_| CheckpointError::CollectionBound)?;
    output.extend_from_slice(&length.to_le_bytes());
    Ok(())
}

fn encode_checkpoint_string(output: &mut Vec<u8>, value: &str) -> Result<(), CheckpointError> {
    if value.len() > MAX_CHECKPOINT_STRING {
        return Err(CheckpointError::CollectionBound);
    }
    let length = u16::try_from(value.len()).map_err(|_| CheckpointError::CollectionBound)?;
    output.extend_from_slice(&length.to_le_bytes());
    output.extend_from_slice(value.as_bytes());
    Ok(())
}

fn encode_checkpoint_levels(
    output: &mut Vec<u8>,
    levels: &BTreeMap<PriceMicros, QuantityMicros>,
) -> Result<(), CheckpointError> {
    if levels.len() > MAX_CHECKPOINT_LEVELS {
        return Err(CheckpointError::CollectionBound);
    }
    encode_u32_length(output, levels.len())?;
    for (price, quantity) in levels {
        output.extend_from_slice(&price.as_micros().to_le_bytes());
        output.extend_from_slice(&quantity.as_micros().to_le_bytes());
    }
    Ok(())
}

fn encode_optional_price_bytes(output: &mut Vec<u8>, value: Option<PriceMicros>) {
    match value {
        Some(value) => {
            output.push(1);
            output.extend_from_slice(&value.as_micros().to_le_bytes());
        }
        None => output.push(0),
    }
}

fn decode_checkpoint_payload(payload: &[u8]) -> Result<ReplayState, CheckpointError> {
    let mut cursor = CheckpointCursor::new(payload);
    let epoch = cursor.u64()?;
    let status = match cursor.u8()? {
        0 => EpochStatus::Inactive,
        1 => EpochStatus::CollectingSnapshots,
        2 => EpochStatus::Synchronized,
        3 => EpochStatus::Shutdown,
        value => return Err(CheckpointError::InvalidStatus(value)),
    };
    let last_sequence = cursor.optional_u64()?;
    let book_count = cursor.u32_usize()?;
    if book_count > MAX_CHECKPOINT_BOOKS {
        return Err(CheckpointError::CollectionBound);
    }
    let mut books = BTreeMap::new();
    for _ in 0..book_count {
        let key = BookKey {
            condition_id: cursor.string()?,
            asset_id: cursor.string()?,
        };
        let authoritative = cursor.boolean()?;
        let bids = cursor.levels()?;
        let asks = cursor.levels()?;
        if authoritative && validate_not_crossed(&bids, &asks).is_err() {
            return Err(CheckpointError::CrossedBook);
        }
        let tick_size = cursor.optional_price()?;
        let last_trade = if cursor.boolean()? {
            let side = match cursor.u8()? {
                1 => MarketSide::Bid,
                2 => MarketSide::Ask,
                value => return Err(CheckpointError::InvalidSide(value)),
            };
            let price = cursor.price()?;
            let quantity = if cursor.boolean()? {
                Some(cursor.quantity()?)
            } else {
                None
            };
            Some((side, price, quantity))
        } else {
            None
        };
        let book = TokenBook {
            bids,
            asks,
            tick_size,
            last_trade,
            authoritative,
        };
        if books.insert(key, book).is_some() {
            return Err(CheckpointError::Duplicate);
        }
    }
    if !cursor.is_finished() {
        return Err(CheckpointError::TrailingBytes);
    }
    Ok(ReplayState {
        epoch,
        status,
        last_sequence,
        books,
    })
}

#[derive(Debug)]
struct CheckpointCursor<'a> {
    bytes: &'a [u8],
    offset: usize,
}

impl<'a> CheckpointCursor<'a> {
    const fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, offset: 0 }
    }

    fn take(&mut self, length: usize) -> Result<&'a [u8], CheckpointError> {
        let end = self
            .offset
            .checked_add(length)
            .ok_or(CheckpointError::InvalidLength)?;
        let value = self
            .bytes
            .get(self.offset..end)
            .ok_or(CheckpointError::Truncated)?;
        self.offset = end;
        Ok(value)
    }

    fn u8(&mut self) -> Result<u8, CheckpointError> {
        Ok(self.take(1)?[0])
    }

    fn u16_usize(&mut self) -> Result<usize, CheckpointError> {
        let bytes = self.take(2)?;
        Ok(usize::from(u16::from_le_bytes([bytes[0], bytes[1]])))
    }

    fn u32_usize(&mut self) -> Result<usize, CheckpointError> {
        let bytes = self.take(4)?;
        usize::try_from(u32::from_le_bytes(
            bytes.try_into().map_err(|_| CheckpointError::Truncated)?,
        ))
        .map_err(|_| CheckpointError::InvalidLength)
    }

    fn i64(&mut self) -> Result<i64, CheckpointError> {
        Ok(i64::from_le_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| CheckpointError::Truncated)?,
        ))
    }

    fn u64(&mut self) -> Result<u64, CheckpointError> {
        Ok(u64::from_le_bytes(
            self.take(8)?
                .try_into()
                .map_err(|_| CheckpointError::Truncated)?,
        ))
    }

    fn boolean(&mut self) -> Result<bool, CheckpointError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            value => Err(CheckpointError::InvalidBoolean(value)),
        }
    }

    fn optional_u64(&mut self) -> Result<Option<u64>, CheckpointError> {
        if self.boolean()? {
            Ok(Some(self.u64()?))
        } else {
            Ok(None)
        }
    }

    fn string(&mut self) -> Result<String, CheckpointError> {
        let length = self.u16_usize()?;
        if length > MAX_CHECKPOINT_STRING {
            return Err(CheckpointError::CollectionBound);
        }
        std::str::from_utf8(self.take(length)?)
            .map(str::to_owned)
            .map_err(|_| CheckpointError::InvalidUtf8)
    }

    fn price(&mut self) -> Result<PriceMicros, CheckpointError> {
        PriceMicros::new(self.i64()?).map_err(|_| CheckpointError::InvalidFinancialValue)
    }

    fn quantity(&mut self) -> Result<QuantityMicros, CheckpointError> {
        QuantityMicros::new(self.i64()?).map_err(|_| CheckpointError::InvalidFinancialValue)
    }

    fn optional_price(&mut self) -> Result<Option<PriceMicros>, CheckpointError> {
        if self.boolean()? {
            Ok(Some(self.price()?))
        } else {
            Ok(None)
        }
    }

    fn levels(&mut self) -> Result<BTreeMap<PriceMicros, QuantityMicros>, CheckpointError> {
        let count = self.u32_usize()?;
        if count > MAX_CHECKPOINT_LEVELS {
            return Err(CheckpointError::CollectionBound);
        }
        let mut levels = BTreeMap::new();
        for _ in 0..count {
            let price = self.price()?;
            let quantity = self.quantity()?;
            if quantity == QuantityMicros::ZERO {
                return Err(CheckpointError::ZeroLevel);
            }
            if levels.insert(price, quantity).is_some() {
                return Err(CheckpointError::Duplicate);
            }
        }
        Ok(levels)
    }

    const fn is_finished(&self) -> bool {
        self.offset == self.bytes.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use market_recorder::{JournalWriter, SegmentConfig, SegmentedJournalWriter};
    use serde_json::{json, Value};
    use tempfile::tempdir;

    const ASSET: &str = "11";

    fn condition() -> String {
        format!("0x{}", "a".repeat(64))
    }

    fn system(sequence: u64, payload: &[u8]) -> EventEnvelope {
        EventEnvelope::new(
            EventSource::System,
            sequence,
            1,
            1,
            SYSTEM_MARKET_ID.to_owned(),
            payload.to_vec(),
        )
        .expect("system event")
    }

    fn market(sequence: u64, kind: u8, assets: &[&str], value: &Value) -> EventEnvelope {
        let timestamp = value
            .get("timestamp")
            .and_then(Value::as_str)
            .expect("timestamp")
            .parse::<i64>()
            .expect("timestamp number");
        let json = serde_json::to_vec(&value).expect("json");
        let mut payload = Vec::new();
        payload.extend_from_slice(&1_u16.to_le_bytes());
        payload.push(kind);
        payload.push(0);
        payload.extend_from_slice(&timestamp.to_le_bytes());
        payload.extend_from_slice(
            &u16::try_from(assets.len())
                .expect("asset count")
                .to_le_bytes(),
        );
        for asset in assets {
            payload.extend_from_slice(
                &u16::try_from(asset.len())
                    .expect("asset length")
                    .to_le_bytes(),
            );
            payload.extend_from_slice(asset.as_bytes());
        }
        payload.extend_from_slice(
            &u32::try_from(json.len())
                .expect("json length")
                .to_le_bytes(),
        );
        payload.extend_from_slice(&json);
        EventEnvelope::new(
            EventSource::Market,
            sequence,
            timestamp * 1_000_000,
            timestamp * 1_000_000 + 1,
            condition(),
            payload,
        )
        .expect("market event")
    }

    fn book(sequence: u64, bid: &str, ask: &str) -> EventEnvelope {
        market(
            sequence,
            1,
            &[ASSET],
            &json!({
                "event_type": "book",
                "market": condition(),
                "asset_id": ASSET,
                "bids": [{"price": bid, "size": "2"}],
                "asks": [{"price": ask, "size": "3"}],
                "timestamp": "1000"
            }),
        )
    }

    fn change(sequence: u64, side: &str, price: &str, quantity: &str) -> EventEnvelope {
        market(
            sequence,
            2,
            &[ASSET],
            &json!({
                "event_type": "price_change",
                "market": condition(),
                "price_changes": [{
                    "asset_id": ASSET,
                    "side": side,
                    "price": price,
                    "size": quantity
                }],
                "timestamp": "1000"
            }),
        )
    }

    #[test]
    fn snapshot_and_deltas_reconstruct_deterministically() {
        let events = [
            system(0, EPOCH_START),
            book(1, "0.40", "0.60"),
            change(2, "BUY", "0.45", "4"),
            change(3, "BUY", "0.45", "5"),
            change(4, "BUY", "0.40", "0"),
            system(5, EPOCH_SYNCED),
        ];
        let mut first = ReplayState::default();
        let mut second = ReplayState::default();
        for event in &events {
            first.apply(event).expect("first replay");
            second.apply(event).expect("second replay");
        }

        assert_eq!(first, second);
        assert_eq!(first.digest(), second.digest());
        assert_eq!(first.status(), EpochStatus::Synchronized);
        let book = first.books().values().next().expect("book");
        assert_eq!(book.bid_levels(), 1);
        assert_eq!(book.best_bid().expect("best bid").0.as_micros(), 450_000);
        assert_eq!(book.best_bid().expect("best bid").1.as_micros(), 5_000_000);
    }

    #[test]
    fn sequence_and_epoch_failures_leave_state_unchanged() {
        let mut state = ReplayState::default();
        state.apply(&system(10, EPOCH_START)).expect("start");
        let before = state.clone();
        assert!(matches!(
            state.apply(&book(12, "0.40", "0.60")),
            Err(ReplayError::SequenceGap { .. })
        ));
        assert_eq!(state, before);

        assert!(matches!(
            state.apply(&change(11, "BUY", "0.45", "2")),
            Err(ReplayError::DeltaBeforeSnapshot(_))
        ));
        assert_eq!(state, before);
    }

    #[test]
    fn rejects_crossed_duplicate_and_imprecise_books() {
        let mut crossed = ReplayState::default();
        crossed.apply(&system(0, EPOCH_START)).expect("start");
        assert!(matches!(
            crossed.apply(&book(1, "0.60", "0.50")),
            Err(ReplayError::CrossedBook)
        ));

        let duplicate = market(
            1,
            1,
            &[ASSET],
            &json!({
                "event_type": "book",
                "market": condition(),
                "asset_id": ASSET,
                "bids": [
                    {"price": "0.4", "size": "1"},
                    {"price": "0.40", "size": "2"}
                ],
                "asks": [],
                "timestamp": "1000"
            }),
        );
        assert!(matches!(
            crossed.apply(&duplicate),
            Err(ReplayError::DuplicateLevel)
        ));

        let imprecise = book(1, "0.4000001", "0.60");
        assert!(matches!(
            crossed.apply(&imprecise),
            Err(ReplayError::Payload(PayloadError::Decimal { .. }))
        ));
    }

    #[test]
    fn prefix_json_identity_mismatch_is_rejected() {
        let mismatch = market(
            1,
            1,
            &["22"],
            &json!({
                "event_type": "book",
                "market": condition(),
                "asset_id": ASSET,
                "bids": [],
                "asks": [],
                "timestamp": "1000"
            }),
        );
        let mut state = ReplayState::default();
        state.apply(&system(0, EPOCH_START)).expect("start");
        assert!(matches!(
            state.apply(&mismatch),
            Err(ReplayError::Payload(PayloadError::Mismatch("asset IDs")))
        ));
    }

    #[test]
    fn tick_trade_and_best_price_events_are_typed_and_applied() {
        let tick = market(
            2,
            3,
            &[ASSET],
            &json!({
                "event_type": "tick_size_change",
                "market": condition(),
                "asset_id": ASSET,
                "old_tick_size": "0.01",
                "new_tick_size": "0.001",
                "timestamp": "1000"
            }),
        );
        let trade = market(
            3,
            4,
            &[ASSET],
            &json!({
                "event_type": "last_trade_price",
                "market": condition(),
                "asset_id": ASSET,
                "side": "BUY",
                "price": "0.50",
                "size": "1.25",
                "timestamp": "1000"
            }),
        );
        let best = market(
            4,
            5,
            &[ASSET],
            &json!({
                "event_type": "best_bid_ask",
                "market": condition(),
                "asset_id": ASSET,
                "best_bid": "0.40",
                "best_ask": "0.60",
                "timestamp": "1000"
            }),
        );

        let mut state = ReplayState::default();
        state.apply(&system(0, EPOCH_START)).expect("start");
        state.apply(&book(1, "0.40", "0.60")).expect("book");
        state.apply(&tick).expect("tick");
        state.apply(&trade).expect("trade");
        state.apply(&best).expect("best bid ask");

        let mismatched_tick = market(
            5,
            3,
            &[ASSET],
            &json!({
                "event_type": "tick_size_change",
                "market": condition(),
                "asset_id": ASSET,
                "old_tick_size": "0.01",
                "new_tick_size": "0.0001",
                "timestamp": "1000"
            }),
        );
        let before = state.clone();
        assert!(matches!(
            state.apply(&mismatched_tick),
            Err(ReplayError::TickSizeMismatch)
        ));
        assert_eq!(state, before);
    }

    #[test]
    fn crossed_delta_requires_a_fresh_snapshot() {
        let mut state = ReplayState::default();
        state.apply(&system(0, EPOCH_START)).expect("start");
        state.apply(&book(1, "0.40", "0.60")).expect("book");
        state.apply(&system(2, EPOCH_SYNCED)).expect("synced");
        assert!(state.is_authoritative());

        state
            .apply(&change(3, "BUY", "0.70", "2"))
            .expect("crossed delta is recorded as non-authoritative");
        assert!(!state.is_authoritative());
        assert!(!state
            .books()
            .values()
            .next()
            .expect("book")
            .is_authoritative());

        state
            .apply(&change(4, "BUY", "0.70", "0"))
            .expect("delta cannot restore authority");
        assert!(!state.is_authoritative());
        state.apply(&book(5, "0.40", "0.60")).expect("snapshot");
        assert!(state.is_authoritative());
    }

    #[test]
    fn journal_replay_has_stable_digest() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("books.journal");
        let events = [
            system(0, EPOCH_START),
            book(1, "0.40", "0.60"),
            change(2, "SELL", "0.60", "4"),
            system(3, EPOCH_SYNCED),
            system(4, EPOCH_SHUTDOWN),
        ];
        let mut writer = JournalWriter::open(&path).expect("writer");
        for event in &events {
            writer.append(event).expect("append");
        }
        writer.sync().expect("sync");
        drop(writer);

        let first = replay_path(&path).expect("first replay");
        let second = replay_path(&path).expect("second replay");
        assert_eq!(first, second);
        assert_eq!(first.digest(), second.digest());
        assert_eq!(first.status(), EpochStatus::Shutdown);
        assert_eq!(first.books().len(), 1);
    }

    #[test]
    fn checkpoint_round_trip_and_checksum_are_strict() {
        let mut state = ReplayState::default();
        state.apply(&system(0, EPOCH_START)).expect("start");
        state.apply(&book(1, "0.40", "0.60")).expect("book");
        state.apply(&system(2, EPOCH_SYNCED)).expect("synced");
        let encoded = state.encode_checkpoint().expect("encode");
        let decoded = ReplayState::decode_checkpoint(&encoded).expect("decode");
        assert_eq!(decoded, state);
        assert_eq!(decoded.digest(), state.digest());

        let mut corrupted = encoded;
        let last = corrupted.last_mut().expect("checksum byte");
        *last ^= 0xff;
        assert!(matches!(
            ReplayState::decode_checkpoint(&corrupted),
            Err(CheckpointError::ChecksumMismatch)
        ));
    }

    #[test]
    fn checkpoint_file_is_create_new_and_bounded() {
        let directory = tempdir().expect("tempdir");
        let path = directory.path().join("state.checkpoint");
        let state = ReplayState::default();
        write_checkpoint(&path, &state).expect("write");
        assert_eq!(read_checkpoint(&path).expect("read"), state);
        assert!(matches!(
            write_checkpoint(&path, &state),
            Err(CheckpointError::Io(_))
        ));
    }

    #[test]
    fn segmented_replay_and_checkpoint_match_single_file() {
        let directory = tempdir().expect("tempdir");
        let single_path = directory.path().join("single.journal");
        let segment_path = directory.path().join("segments");
        let events = [
            system(0, EPOCH_START),
            book(1, "0.40", "0.60"),
            change(2, "SELL", "0.60", "4"),
            system(3, EPOCH_SYNCED),
            system(4, EPOCH_SHUTDOWN),
        ];
        let mut single = JournalWriter::open(&single_path).expect("single");
        let mut segmented = SegmentedJournalWriter::open(
            &segment_path,
            SegmentConfig {
                max_segment_bytes: u64::MAX,
                max_segment_records: 2,
            },
        )
        .expect("segmented");
        let mut checkpoint_state = ReplayState::default();
        for (index, event) in events.iter().enumerate() {
            single.append(event).expect("single append");
            segmented.append(event).expect("segment append");
            if index <= 2 {
                checkpoint_state.apply(event).expect("checkpoint prefix");
            }
        }
        single.sync().expect("single sync");
        segmented.sync().expect("segment sync");
        drop(single);
        drop(segmented);

        let expected = replay_path(single_path).expect("single replay");
        let actual = replay_segmented_path(&segment_path).expect("segmented replay");
        let resumed = replay_segmented_from_checkpoint(&segment_path, &checkpoint_state)
            .expect("checkpoint replay");
        assert_eq!(actual, expected);
        assert_eq!(resumed, expected);

        let mut wrong = checkpoint_state;
        wrong.epoch = 99;
        assert!(matches!(
            replay_segmented_from_checkpoint(segment_path, &wrong),
            Err(ReplayError::CheckpointMismatch)
        ));
    }
}
