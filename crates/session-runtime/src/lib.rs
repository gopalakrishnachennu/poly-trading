#![forbid(unsafe_code)]

//! Durable, bounded, read-only runtime for hourly session coordination.

mod codec;

pub use codec::{decode as decode_command, encode as encode_command, CodecError, DurableCommand};

use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{
    EventJournal, JournalBackendError, SegmentConfig, SegmentError, SegmentedJournalReader,
    SegmentedJournalWriter,
};
use market_session::{
    CoordinationFrame, CoordinatorError, CoordinatorSnapshot, MarketSessionCoordinator,
};
use public_market_data::MarketIdentity;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use thiserror::Error;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

const SESSION_STREAM_ID: &str = "__hourly_session_coordinator__";
const CHECKPOINT_MAGIC: &[u8; 8] = b"POLYSSP1";
const CHECKPOINT_VERSION: u16 = 1;
const CHECKPOINT_BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;

#[derive(Clone, Copy, Debug)]
pub struct RuntimeConfig {
    pub channel_capacity: usize,
    pub segment: SegmentConfig,
}

impl Default for RuntimeConfig {
    fn default() -> Self {
        Self {
            channel_capacity: 1_024,
            segment: SegmentConfig::default(),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeMode {
    Ready,
    Halted,
    Closed,
    Shutdown,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RuntimeSnapshot {
    pub mode: RuntimeMode,
    pub last_sequence: Option<u64>,
    pub coordinator: CoordinatorSnapshot,
    pub halt_reason: Option<String>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrefixCheckpoint {
    pub sequence: u64,
    pub coordinator_digest: [u8; 32],
}

#[derive(Debug)]
pub struct RecoveryState {
    pub coordinator: MarketSessionCoordinator,
    pub last_sequence: Option<u64>,
}

#[derive(Debug)]
pub struct DurableCoordinator<J> {
    journal: J,
    coordinator: MarketSessionCoordinator,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableCoordinator<J> {
    /// Creates a journal-first coordinator after checking recovery alignment.
    ///
    /// # Errors
    ///
    /// Returns [`RuntimeError::RecoveryMismatch`] unless journal and replay
    /// state end at the same sequence.
    pub fn new(journal: J, recovery: RecoveryState) -> Result<Self, RuntimeError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(RuntimeError::RecoveryMismatch);
        }
        Ok(Self {
            journal,
            coordinator: recovery.coordinator,
            last_sequence: recovery.last_sequence,
            poisoned: None,
        })
    }

    /// Device-syncs a command before applying the state transition.
    ///
    /// # Errors
    ///
    /// Journal, sync, encoding, sequence, or coordinator failures poison this
    /// live instance. Restart recovery determines the durable truth.
    pub fn apply(&mut self, command: DurableCommand) -> Result<CoordinatorSnapshot, RuntimeError> {
        if let Some(reason) = &self.poisoned {
            return Err(RuntimeError::Halted(reason.clone()));
        }
        let sequence = self.last_sequence.map_or(Ok(0), |value| {
            value.checked_add(1).ok_or(RuntimeError::SequenceExhausted)
        })?;
        let envelope = command_envelope(sequence, &command)?;
        if let Err(error) = self.journal.append_event(&envelope) {
            return self.poison(RuntimeError::Journal(error));
        }
        self.last_sequence = Some(sequence);
        if let Err(error) = self.journal.sync_events() {
            return self.poison(RuntimeError::Journal(error));
        }
        let result = apply_command(&mut self.coordinator, command);
        match result {
            Ok(snapshot) => Ok(snapshot),
            Err(error) => self.poison(RuntimeError::Coordinator(error)),
        }
    }

    /// Synchronizes the journal without changing coordinator state.
    ///
    /// # Errors
    ///
    /// Returns a journal error and poisons the instance on failure.
    pub fn sync(&mut self) -> Result<(), RuntimeError> {
        if let Some(reason) = &self.poisoned {
            return Err(RuntimeError::Halted(reason.clone()));
        }
        self.journal
            .sync_events()
            .map_err(RuntimeError::Journal)
            .or_else(|error| self.poison(error))
    }

    #[must_use]
    pub const fn last_sequence(&self) -> Option<u64> {
        self.last_sequence
    }

    #[must_use]
    pub fn snapshot(&self) -> CoordinatorSnapshot {
        self.coordinator.snapshot()
    }

    #[must_use]
    pub const fn journal(&self) -> &J {
        &self.journal
    }

    fn poison<T>(&mut self, error: RuntimeError) -> Result<T, RuntimeError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

fn command_envelope(
    sequence: u64,
    command: &DurableCommand,
) -> Result<EventEnvelope, RuntimeError> {
    let timestamp = command.timestamp_ns();
    if timestamp < 0 {
        return Err(RuntimeError::Codec(CodecError::Timestamp));
    }
    Ok(EventEnvelope::new(
        EventSource::System,
        sequence,
        timestamp,
        timestamp,
        SESSION_STREAM_ID.to_owned(),
        encode_command(command)?,
    )?)
}

fn apply_command(
    coordinator: &mut MarketSessionCoordinator,
    command: DurableCommand,
) -> Result<CoordinatorSnapshot, CoordinatorError> {
    match command {
        DurableCommand::Register { identity, .. } => {
            coordinator.register(identity)?;
            Ok(coordinator.snapshot())
        }
        DurableCommand::Coordinate(frame) => coordinator.evaluate(&frame),
    }
}

/// Replays a complete segmented session journal and optionally verifies a
/// durable-prefix checkpoint.
///
/// # Errors
///
/// Rejects directory, segment, sequence, envelope, payload, state, and
/// checkpoint inconsistencies.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    checkpoint: Option<PrefixCheckpoint>,
) -> Result<RecoveryState, RuntimeError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut coordinator = MarketSessionCoordinator::default();
    let mut expected = 0_u64;
    let mut last_sequence = None;
    let mut checkpoint_verified = checkpoint.is_none();
    while let Some(envelope) = reader.next_event()? {
        if envelope.sequence != expected {
            return Err(RuntimeError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(RuntimeError::SequenceExhausted)?;
        let command = validate_envelope(&envelope)?;
        apply_command(&mut coordinator, command)?;
        last_sequence = Some(envelope.sequence);
        if checkpoint.is_some_and(|value| value.sequence == envelope.sequence) {
            let expected_digest = checkpoint
                .map(|value| value.coordinator_digest)
                .ok_or(RuntimeError::CheckpointMismatch)?;
            if coordinator.snapshot().digest != expected_digest {
                return Err(RuntimeError::CheckpointMismatch);
            }
            checkpoint_verified = true;
        }
    }
    if !checkpoint_verified {
        return Err(RuntimeError::CheckpointSequenceMissing);
    }
    Ok(RecoveryState {
        coordinator,
        last_sequence,
    })
}

fn validate_envelope(envelope: &EventEnvelope) -> Result<DurableCommand, RuntimeError> {
    if envelope.source != EventSource::System || envelope.market_id != SESSION_STREAM_ID {
        return Err(RuntimeError::EnvelopeIdentity);
    }
    let command = decode_command(&envelope.payload)?;
    if envelope.event_time_ns != command.timestamp_ns()
        || envelope.received_time_ns != command.timestamp_ns()
    {
        return Err(RuntimeError::EnvelopeTimestamp);
    }
    Ok(command)
}

/// Writes a new checksummed durable-prefix checkpoint without replacement.
///
/// # Errors
///
/// Returns an error for existing targets or I/O failures.
pub fn write_checkpoint(
    path: impl AsRef<Path>,
    checkpoint: PrefixCheckpoint,
) -> Result<(), CheckpointError> {
    let bytes = encode_checkpoint(checkpoint);
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Reads and validates one exact checkpoint file.
///
/// # Errors
///
/// Rejects wrong size, magic, version, reserved bytes, checksum, and I/O.
pub fn read_checkpoint(path: impl AsRef<Path>) -> Result<PrefixCheckpoint, CheckpointError> {
    decode_checkpoint(&fs::read(path)?)
}

fn encode_checkpoint(checkpoint: PrefixCheckpoint) -> [u8; CHECKPOINT_BYTES] {
    let mut bytes = [0_u8; CHECKPOINT_BYTES];
    bytes[0..8].copy_from_slice(CHECKPOINT_MAGIC);
    bytes[8..10].copy_from_slice(&CHECKPOINT_VERSION.to_le_bytes());
    bytes[16..24].copy_from_slice(&checkpoint.sequence.to_le_bytes());
    bytes[24..56].copy_from_slice(&checkpoint.coordinator_digest);
    let checksum = blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]);
    bytes[CHECKPOINT_BODY_BYTES..].copy_from_slice(checksum.as_bytes());
    bytes
}

fn decode_checkpoint(bytes: &[u8]) -> Result<PrefixCheckpoint, CheckpointError> {
    if bytes.len() != CHECKPOINT_BYTES {
        return Err(CheckpointError::Length);
    }
    if bytes.get(0..8) != Some(CHECKPOINT_MAGIC) {
        return Err(CheckpointError::Magic);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| CheckpointError::Length)?,
    );
    if version != CHECKPOINT_VERSION {
        return Err(CheckpointError::Version(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(CheckpointError::Reserved);
    }
    if blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]).as_bytes() != &bytes[CHECKPOINT_BODY_BYTES..] {
        return Err(CheckpointError::Checksum);
    }
    Ok(PrefixCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| CheckpointError::Length)?,
        ),
        coordinator_digest: bytes[24..56]
            .try_into()
            .map_err(|_| CheckpointError::Length)?,
    })
}

#[derive(Clone, Debug)]
enum RuntimeCommand {
    Apply(Box<DurableCommand>),
    Checkpoint(PathBuf),
    Shutdown,
}

#[derive(Clone, Debug)]
pub struct RuntimeIngress {
    sender: mpsc::Sender<RuntimeCommand>,
}

impl RuntimeIngress {
    /// Non-blocking bounded registration submission.
    ///
    /// # Errors
    ///
    /// Returns full or closed without dropping into an unbounded queue.
    pub fn try_register(
        &self,
        identity: MarketIdentity,
        recorded_at_ns: i64,
    ) -> Result<(), IngressError> {
        self.try_send(RuntimeCommand::Apply(Box::new(DurableCommand::Register {
            identity,
            recorded_at_ns,
        })))
    }

    /// Non-blocking bounded frame submission.
    ///
    /// # Errors
    ///
    /// Returns full or closed without silently dropping the frame.
    pub fn try_coordinate(&self, frame: CoordinationFrame) -> Result<(), IngressError> {
        self.try_send(RuntimeCommand::Apply(Box::new(DurableCommand::Coordinate(
            frame,
        ))))
    }

    /// Requests a create-new checkpoint at the current durable prefix.
    ///
    /// # Errors
    ///
    /// Returns full or closed when the request cannot enter the bounded actor.
    pub fn try_checkpoint(&self, path: PathBuf) -> Result<(), IngressError> {
        self.try_send(RuntimeCommand::Checkpoint(path))
    }

    /// Requests clean synchronized shutdown.
    ///
    /// # Errors
    ///
    /// Returns full or closed when the request cannot enter the bounded actor.
    pub fn try_shutdown(&self) -> Result<(), IngressError> {
        self.try_send(RuntimeCommand::Shutdown)
    }

    fn try_send(&self, command: RuntimeCommand) -> Result<(), IngressError> {
        self.sender.try_send(command).map_err(|error| match error {
            mpsc::error::TrySendError::Full(_) => IngressError::Full,
            mpsc::error::TrySendError::Closed(_) => IngressError::Closed,
        })
    }
}

#[derive(Debug)]
pub struct SessionRuntime {
    pub ingress: RuntimeIngress,
    pub snapshots: watch::Receiver<RuntimeSnapshot>,
    pub task: JoinHandle<RuntimeSnapshot>,
}

/// Recovers durable state, opens the segmented writer, and spawns one owner.
///
/// # Errors
///
/// Rejects invalid configuration, directory/journal/checkpoint corruption, or
/// recovery/writer disagreement before any task is spawned.
pub fn spawn_runtime(
    directory: impl AsRef<Path>,
    checkpoint: Option<PrefixCheckpoint>,
    config: RuntimeConfig,
) -> Result<SessionRuntime, RuntimeError> {
    if config.channel_capacity == 0 {
        return Err(RuntimeError::InvalidConfig);
    }
    let directory = directory.as_ref().to_path_buf();
    let recovery = if directory_has_entries(&directory)? {
        recover_segmented(&directory, checkpoint)?
    } else {
        if checkpoint.is_some() {
            return Err(RuntimeError::CheckpointSequenceMissing);
        }
        RecoveryState {
            coordinator: MarketSessionCoordinator::default(),
            last_sequence: None,
        }
    };
    let writer = SegmentedJournalWriter::open(&directory, config.segment)?;
    let durable = DurableCoordinator::new(writer, recovery)?;
    let initial = RuntimeSnapshot {
        mode: RuntimeMode::Ready,
        last_sequence: durable.last_sequence(),
        coordinator: durable.snapshot(),
        halt_reason: None,
    };
    let (sender, mut receiver) = mpsc::channel(config.channel_capacity);
    let (snapshot_sender, snapshots) = watch::channel(initial);
    let task = tokio::spawn(async move {
        let mut durable = durable;
        loop {
            let Some(command) = receiver.recv().await else {
                return terminal_sync(&mut durable, RuntimeMode::Closed, &snapshot_sender);
            };
            match command {
                RuntimeCommand::Apply(command) => match durable.apply(*command) {
                    Ok(coordinator) => {
                        snapshot_sender.send_replace(RuntimeSnapshot {
                            mode: RuntimeMode::Ready,
                            last_sequence: durable.last_sequence(),
                            coordinator,
                            halt_reason: None,
                        });
                    }
                    Err(error) => {
                        let snapshot = halted_snapshot(&durable, error.to_string());
                        snapshot_sender.send_replace(snapshot.clone());
                        return snapshot;
                    }
                },
                RuntimeCommand::Checkpoint(path) => {
                    let Some(sequence) = durable.last_sequence() else {
                        let snapshot =
                            halted_snapshot(&durable, RuntimeError::EmptyCheckpoint.to_string());
                        snapshot_sender.send_replace(snapshot.clone());
                        return snapshot;
                    };
                    let checkpoint = PrefixCheckpoint {
                        sequence,
                        coordinator_digest: durable.snapshot().digest,
                    };
                    if let Err(error) = write_checkpoint(path, checkpoint) {
                        let snapshot = halted_snapshot(&durable, error.to_string());
                        snapshot_sender.send_replace(snapshot.clone());
                        return snapshot;
                    }
                }
                RuntimeCommand::Shutdown => {
                    return terminal_sync(&mut durable, RuntimeMode::Shutdown, &snapshot_sender);
                }
            }
        }
    });
    Ok(SessionRuntime {
        ingress: RuntimeIngress { sender },
        snapshots,
        task,
    })
}

fn terminal_sync(
    durable: &mut DurableCoordinator<SegmentedJournalWriter>,
    mode: RuntimeMode,
    sender: &watch::Sender<RuntimeSnapshot>,
) -> RuntimeSnapshot {
    let snapshot = match durable.sync() {
        Ok(()) => RuntimeSnapshot {
            mode,
            last_sequence: durable.last_sequence(),
            coordinator: durable.snapshot(),
            halt_reason: None,
        },
        Err(error) => halted_snapshot(durable, error.to_string()),
    };
    sender.send_replace(snapshot.clone());
    snapshot
}

fn halted_snapshot<J: EventJournal>(
    durable: &DurableCoordinator<J>,
    reason: String,
) -> RuntimeSnapshot {
    RuntimeSnapshot {
        mode: RuntimeMode::Halted,
        last_sequence: durable.last_sequence(),
        coordinator: durable.snapshot(),
        halt_reason: Some(reason),
    }
}

fn directory_has_entries(path: &Path) -> Result<bool, RuntimeError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => {
            if !metadata.is_dir() || metadata.file_type().is_symlink() {
                return Err(RuntimeError::InvalidDirectory);
            }
            Ok(fs::read_dir(path)?.next().transpose()?.is_some())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(RuntimeError::Io(error)),
    }
}

#[derive(Clone, Copy, Debug, Error, Eq, PartialEq)]
pub enum IngressError {
    #[error("session runtime ingress is full")]
    Full,
    #[error("session runtime ingress is closed")]
    Closed,
}

#[derive(Debug, Error)]
pub enum RuntimeError {
    #[error("session runtime configuration is invalid")]
    InvalidConfig,
    #[error("session runtime directory is invalid or symbolic")]
    InvalidDirectory,
    #[error("session runtime I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("session command codec error: {0}")]
    Codec(#[from] CodecError),
    #[error("session envelope error: {0:?}")]
    Schema(#[from] SchemaError),
    #[error("session journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("session segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("session coordinator error: {0}")]
    Coordinator(#[from] CoordinatorError),
    #[error("session journal sequence mismatch: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("session journal sequence is exhausted")]
    SequenceExhausted,
    #[error("session envelope source or identity is invalid")]
    EnvelopeIdentity,
    #[error("session envelope timestamps do not match its command")]
    EnvelopeTimestamp,
    #[error("journal writer and recovered state disagree")]
    RecoveryMismatch,
    #[error("checkpoint digest does not match its durable prefix")]
    CheckpointMismatch,
    #[error("checkpoint sequence is absent from the durable journal")]
    CheckpointSequenceMissing,
    #[error("cannot checkpoint an empty session journal")]
    EmptyCheckpoint,
    #[error("session runtime is halted: {0}")]
    Halted(String),
}

#[derive(Debug, Error)]
pub enum CheckpointError {
    #[error("session checkpoint I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("session checkpoint length is invalid")]
    Length,
    #[error("session checkpoint magic is invalid")]
    Magic,
    #[error("unsupported session checkpoint version: {0}")]
    Version(u16),
    #[error("session checkpoint reserved bytes are non-zero")]
    Reserved,
    #[error("session checkpoint checksum mismatch")]
    Checksum,
}

#[cfg(test)]
mod tests {
    use super::*;
    use common_types::{PriceMicros, QuantityMicros, QuotePriceMicros, ReferenceQuantityE8};
    use feed_supervisor::{SupervisorMode, SupervisorSnapshot};
    use live_market_state::{ActorMode, ActorSnapshot};
    use market_recorder::{JournalError, SegmentConfig};
    use market_session::{SessionKey, SessionSourceState, TokenBookView};
    use public_market_data::{Asset, BTC_HOURLY};
    use reference_market_data::{
        CandleData, CandleInterval, InProgressCandle, ReferenceHealth, ReferenceSnapshot,
        ReferenceSymbol,
    };
    use std::collections::BTreeMap;
    use tempfile::tempdir;

    const START_MS: i64 = 3_600_000;

    fn identity() -> MarketIdentity {
        MarketIdentity {
            asset: Asset::Bitcoin,
            event_id: "event".to_owned(),
            market_id: "market".to_owned(),
            condition_id: format!("0x{}", "a".repeat(64)),
            question_id: format!("0x{}", "b".repeat(64)),
            event_slug: "event".to_owned(),
            market_slug: "market".to_owned(),
            series_id: BTC_HOURLY.id.to_owned(),
            series_slug: BTC_HOURLY.slug.to_owned(),
            title: "BTC Up or Down".to_owned(),
            start_time_ms: START_MS,
            end_time_ms: 2 * START_MS,
            resolution_source: "https://www.binance.com/en/trade/BTC_USDT".to_owned(),
            description: "This market will resolve to \"Up\" if the close price is greater than or equal to the open price for the BTC/USDT 1 hour candle that begins on the time and date specified in the title. Otherwise, this market will resolve to \"Down\". The close and open displayed for the relevant candle will be used once the data for that candle is finalized. This market is about the Binance pair, not according to other exchanges or trading pairs.".to_owned(),
            up_token_id: "11".to_owned(),
            down_token_id: "22".to_owned(),
            rules_fingerprint: [7; 32],
        }
    }

    fn candle(close: i64) -> CandleData {
        CandleData {
            symbol: ReferenceSymbol::BtcUsdt,
            interval: CandleInterval::OneHourUtc,
            open_time_ms: START_MS,
            close_time_ms: 2 * START_MS - 1,
            first_trade_id: 1,
            last_trade_id: 2,
            open: QuotePriceMicros::new(100_000_000).expect("open"),
            high: QuotePriceMicros::new(110_000_000).expect("high"),
            low: QuotePriceMicros::new(90_000_000).expect("low"),
            close: QuotePriceMicros::new(close).expect("close"),
            base_volume: ReferenceQuantityE8::new(100_000_000).expect("volume"),
            quote_volume: ReferenceQuantityE8::new(10_000_000_000).expect("volume"),
            trade_count: 2,
        }
    }

    fn book() -> TokenBookView {
        TokenBookView {
            authoritative: true,
            best_bid: Some((
                PriceMicros::new(400_000).expect("price"),
                QuantityMicros::new(2_000_000).expect("quantity"),
            )),
            best_ask: Some((
                PriceMicros::new(410_000).expect("price"),
                QuantityMicros::new(3_000_000).expect("quantity"),
            )),
        }
    }

    fn frame(now_ns: i64) -> CoordinationFrame {
        let identity = identity();
        let market_digest = [3; 32];
        let reference_digest = [4; 32];
        CoordinationFrame {
            now_ns,
            market: ActorSnapshot {
                mode: ActorMode::Ready,
                ready: true,
                epoch: 1,
                last_sequence: Some(10),
                book_count: 2,
                digest: market_digest,
                last_market_event_ns: Some(now_ns),
                last_market_received_ns: Some(now_ns),
                halt_reason: None,
            },
            reference: ReferenceSnapshot {
                health: ReferenceHealth::Ready,
                epoch: 2,
                last_sequence: Some(20),
                digest: reference_digest,
                last_reference_received_ns: Some(now_ns),
                symbols: BTreeMap::new(),
            },
            supervision: SupervisorSnapshot {
                mode: SupervisorMode::Ready,
                ready: true,
                evaluated_at_ns: Some(now_ns),
                market_epoch: 1,
                market_sequence: Some(10),
                market_digest: [5; 32],
                market_state_digest: market_digest,
                reference_epoch: 2,
                reference_sequence: Some(20),
                reference_digest: [6; 32],
                reference_state_digest: reference_digest,
                halt_reason: None,
                digest: [8; 32],
            },
            sessions: BTreeMap::from([(
                SessionKey::from(&identity),
                SessionSourceState {
                    up_book: Some(book()),
                    down_book: Some(book()),
                    in_progress: Some(InProgressCandle(candle(101_000_000))),
                    finalized: None,
                },
            )]),
        }
    }

    #[test]
    fn canonical_commands_round_trip_deterministically() {
        let commands = [
            DurableCommand::Register {
                identity: identity(),
                recorded_at_ns: 1,
            },
            DurableCommand::Coordinate(frame(START_MS * 1_000_000)),
        ];
        for command in commands {
            let first = encode_command(&command).expect("encode");
            let decoded = decode_command(&first).expect("decode");
            assert_eq!(decoded, command);
            assert_eq!(encode_command(&decoded).expect("re-encode"), first);
        }
    }

    #[test]
    fn codec_rejects_version_enum_financial_and_trailing_data() {
        let valid = encode_command(&DurableCommand::Coordinate(frame(START_MS * 1_000_000)))
            .expect("encode");
        let mut value: serde_json::Value = serde_json::from_slice(&valid).expect("JSON");
        value["version"] = serde_json::json!(3);
        assert_eq!(
            decode_command(&serde_json::to_vec(&value).expect("JSON")),
            Err(CodecError::Version(3))
        );
        value["version"] = serde_json::json!(2);
        value["command"]["value"]["sessions"][0]["asset"] = serde_json::json!(9);
        assert_eq!(
            decode_command(&serde_json::to_vec(&value).expect("JSON")),
            Err(CodecError::Enum)
        );
        value["command"]["value"]["sessions"][0]["asset"] = serde_json::json!(0);
        value["command"]["value"]["sessions"][0]["up_book"]["best_bid"]["price_micros"] =
            serde_json::json!(2_000_000);
        assert_eq!(
            decode_command(&serde_json::to_vec(&value).expect("JSON")),
            Err(CodecError::Financial)
        );
        let mut trailing = valid;
        trailing.extend_from_slice(b"x");
        assert!(matches!(
            decode_command(&trailing),
            Err(CodecError::Json(_))
        ));
    }

    #[derive(Debug, Default)]
    struct FakeJournal {
        events: Vec<EventEnvelope>,
        fail_append: bool,
        fail_sync: bool,
    }

    impl EventJournal for FakeJournal {
        fn append_event(&mut self, event: &EventEnvelope) -> Result<u64, JournalBackendError> {
            if self.fail_append {
                return Err(JournalBackendError::Single(JournalError::Io(
                    std::io::Error::other("append"),
                )));
            }
            self.events.push(event.clone());
            Ok(0)
        }

        fn sync_events(&self) -> Result<(), JournalBackendError> {
            if self.fail_sync {
                Err(JournalBackendError::Single(JournalError::Io(
                    std::io::Error::other("sync"),
                )))
            } else {
                Ok(())
            }
        }

        fn last_event_sequence(&self) -> Option<u64> {
            self.events.last().map(|event| event.sequence)
        }
    }

    fn empty_recovery() -> RecoveryState {
        RecoveryState {
            coordinator: MarketSessionCoordinator::default(),
            last_sequence: None,
        }
    }

    #[test]
    fn journal_failure_never_mutates_coordinator() {
        let journal = FakeJournal {
            fail_append: true,
            ..FakeJournal::default()
        };
        let mut durable = DurableCoordinator::new(journal, empty_recovery()).expect("durable");
        let before = durable.snapshot();
        assert!(matches!(
            durable.apply(DurableCommand::Register {
                identity: identity(),
                recorded_at_ns: 1,
            }),
            Err(RuntimeError::Journal(_))
        ));
        assert_eq!(durable.snapshot(), before);
        assert_eq!(durable.last_sequence(), None);
        assert!(matches!(
            durable.apply(DurableCommand::Register {
                identity: identity(),
                recorded_at_ns: 2,
            }),
            Err(RuntimeError::Halted(_))
        ));
    }

    #[test]
    fn sync_failure_leaves_state_unapplied_and_poisoned() {
        let journal = FakeJournal {
            fail_sync: true,
            ..FakeJournal::default()
        };
        let mut durable = DurableCoordinator::new(journal, empty_recovery()).expect("durable");
        let before = durable.snapshot();
        assert!(matches!(
            durable.apply(DurableCommand::Register {
                identity: identity(),
                recorded_at_ns: 1,
            }),
            Err(RuntimeError::Journal(_))
        ));
        assert_eq!(durable.snapshot(), before);
        assert_eq!(durable.last_sequence(), Some(0));
        assert_eq!(durable.journal.events.len(), 1);
    }

    #[test]
    fn envelope_source_and_timestamp_are_revalidated_on_replay() {
        let command = DurableCommand::Register {
            identity: identity(),
            recorded_at_ns: 5,
        };
        let mut envelope = command_envelope(0, &command).expect("envelope");
        envelope.source = EventSource::Market;
        assert!(matches!(
            validate_envelope(&envelope),
            Err(RuntimeError::EnvelopeIdentity)
        ));
        envelope.source = EventSource::System;
        envelope.received_time_ns = 6;
        assert!(matches!(
            validate_envelope(&envelope),
            Err(RuntimeError::EnvelopeTimestamp)
        ));
    }

    #[test]
    fn invalid_transition_is_durable_before_terminal_halt() {
        let mut invalid = identity();
        invalid.description = "changed rules".to_owned();
        let mut durable =
            DurableCoordinator::new(FakeJournal::default(), empty_recovery()).expect("durable");
        assert!(matches!(
            durable.apply(DurableCommand::Register {
                identity: invalid,
                recorded_at_ns: 1,
            }),
            Err(RuntimeError::Coordinator(_))
        ));
        assert_eq!(durable.journal.events.len(), 1);
        assert_eq!(durable.last_sequence(), Some(0));
    }

    #[test]
    fn segmented_restart_and_checkpoint_validation_match_online_state() {
        let temp = tempdir().expect("temp");
        let journal_dir = temp.path().join("journal");
        let writer = SegmentedJournalWriter::open(
            &journal_dir,
            SegmentConfig {
                max_segment_bytes: 4_096,
                max_segment_records: 2,
            },
        )
        .expect("writer");
        let mut durable = DurableCoordinator::new(writer, empty_recovery()).expect("durable");
        durable
            .apply(DurableCommand::Register {
                identity: identity(),
                recorded_at_ns: 1,
            })
            .expect("register");
        durable
            .apply(DurableCommand::Coordinate(frame(START_MS * 1_000_000)))
            .expect("frame");
        let checkpoint = PrefixCheckpoint {
            sequence: durable.last_sequence().expect("sequence"),
            coordinator_digest: durable.snapshot().digest,
        };
        let online = durable.snapshot();
        drop(durable);

        let recovered = recover_segmented(&journal_dir, Some(checkpoint)).expect("recover");
        assert_eq!(recovered.coordinator.snapshot(), online);
        let writer =
            SegmentedJournalWriter::open(&journal_dir, SegmentConfig::default()).expect("reopen");
        let mut restarted = DurableCoordinator::new(writer, recovered).expect("restart");
        restarted
            .apply(DurableCommand::Coordinate(frame(START_MS * 1_000_000 + 1)))
            .expect("next frame");
        assert_eq!(restarted.last_sequence(), Some(2));

        let mut wrong = checkpoint;
        wrong.coordinator_digest[0] ^= 1;
        assert!(matches!(
            recover_segmented(&journal_dir, Some(wrong)),
            Err(RuntimeError::CheckpointMismatch)
        ));
        assert!(matches!(
            recover_segmented(
                &journal_dir,
                Some(PrefixCheckpoint {
                    sequence: 99,
                    coordinator_digest: [0; 32],
                })
            ),
            Err(RuntimeError::CheckpointSequenceMissing)
        ));
    }

    #[test]
    fn checkpoint_file_is_create_new_and_checksum_strict() {
        let temp = tempdir().expect("temp");
        let path = temp.path().join("checkpoint");
        let checkpoint = PrefixCheckpoint {
            sequence: 7,
            coordinator_digest: [9; 32],
        };
        write_checkpoint(&path, checkpoint).expect("write");
        assert_eq!(read_checkpoint(&path).expect("read"), checkpoint);
        assert!(matches!(
            write_checkpoint(&path, checkpoint),
            Err(CheckpointError::Io(_))
        ));
        let mut bytes = fs::read(&path).expect("bytes");
        bytes[20] ^= 1;
        assert!(matches!(
            decode_checkpoint(&bytes),
            Err(CheckpointError::Checksum)
        ));
    }

    #[test]
    fn ingress_reports_full_and_closed_without_dropping_silently() {
        let (sender, receiver) = mpsc::channel(1);
        let ingress = RuntimeIngress { sender };
        ingress.try_register(identity(), 1).expect("first fits");
        assert_eq!(ingress.try_register(identity(), 2), Err(IngressError::Full));
        drop(receiver);
        assert_eq!(ingress.try_shutdown(), Err(IngressError::Closed));
    }

    #[tokio::test]
    async fn runtime_shutdown_checkpoint_and_restart_are_deterministic() {
        let temp = tempdir().expect("temp");
        let journal_dir = temp.path().join("journal");
        let checkpoint_path = temp.path().join("checkpoint");
        let runtime = spawn_runtime(
            &journal_dir,
            None,
            RuntimeConfig {
                channel_capacity: 8,
                segment: SegmentConfig::default(),
            },
        )
        .expect("spawn");
        runtime
            .ingress
            .try_register(identity(), 1)
            .expect("register");
        runtime
            .ingress
            .try_coordinate(frame(START_MS * 1_000_000))
            .expect("frame");
        runtime
            .ingress
            .try_checkpoint(checkpoint_path.clone())
            .expect("checkpoint");
        runtime.ingress.try_shutdown().expect("shutdown");
        let terminal = runtime.task.await.expect("join");
        assert_eq!(terminal.mode, RuntimeMode::Shutdown);
        assert_eq!(terminal.last_sequence, Some(1));
        let checkpoint = read_checkpoint(checkpoint_path).expect("checkpoint");
        assert_eq!(checkpoint.coordinator_digest, terminal.coordinator.digest);

        let restarted = spawn_runtime(&journal_dir, Some(checkpoint), RuntimeConfig::default())
            .expect("restart");
        assert_eq!(
            restarted.snapshots.borrow().coordinator.digest,
            terminal.coordinator.digest
        );
        restarted.ingress.try_shutdown().expect("shutdown");
        assert_eq!(
            restarted.task.await.expect("join").mode,
            RuntimeMode::Shutdown
        );
    }

    #[tokio::test]
    async fn invalid_runtime_frame_is_terminal_and_preserves_last_good_state() {
        let temp = tempdir().expect("temp");
        let runtime = spawn_runtime(temp.path().join("journal"), None, RuntimeConfig::default())
            .expect("spawn");
        runtime
            .ingress
            .try_register(identity(), 1)
            .expect("register");
        let mut invalid = frame(START_MS * 1_000_000);
        invalid.supervision.market_state_digest = [99; 32];
        runtime
            .ingress
            .try_coordinate(invalid)
            .expect("invalid frame entered journal");
        let terminal = runtime.task.await.expect("join");
        assert_eq!(terminal.mode, RuntimeMode::Halted);
        assert_eq!(terminal.last_sequence, Some(1));
        assert!(terminal.coordinator.halted);
        assert!(terminal.halt_reason.is_some());
    }
}
