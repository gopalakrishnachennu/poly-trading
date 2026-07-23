use crate::{
    decode_command, encode_command, Error as RiskError, PortfolioRiskEngine, RiskCommand,
    RiskDecision,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

const STREAM_ID: &str = "__portfolio_risk__";
const CHECKPOINT_MAGIC: &[u8; 8] = b"POLYRKP1";
const CHECKPOINT_VERSION: u16 = 1;
const CHECKPOINT_BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RiskCheckpoint {
    pub sequence: u64,
    pub risk_digest: [u8; 32],
}

#[derive(Debug)]
pub struct RiskRecovery {
    pub engine: PortfolioRiskEngine,
    pub last_sequence: Option<u64>,
}

#[derive(Debug)]
pub struct DurableRiskEngine<J> {
    journal: J,
    engine: PortfolioRiskEngine,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableRiskEngine<J> {
    /// Aligns an opened writer with recovered state.
    ///
    /// # Errors
    ///
    /// Rejects writer/recovery sequence disagreement.
    pub fn new(journal: J, recovery: RiskRecovery) -> Result<Self, StorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(StorageError::RecoveryMismatch);
        }
        Ok(Self {
            journal,
            engine: recovery.engine,
            last_sequence: recovery.last_sequence,
            poisoned: None,
        })
    }

    /// Preflights, appends, device-syncs, and only then installs a risk decision
    /// or absorbing integrity halt.
    ///
    /// # Errors
    ///
    /// Returns risk, schema, journal, or sequence errors. Post-append failures
    /// poison the live owner.
    pub fn apply(&mut self, command: &RiskCommand) -> Result<RiskDecision, StorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(StorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut preflight = self.engine.clone();
        let result = preflight.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |value| {
            value.checked_add(1).ok_or(StorageError::SequenceExhausted)
        })?;
        let timestamp = command.recorded_at_ns();
        let envelope = EventEnvelope::new(
            EventSource::System,
            sequence,
            timestamp,
            timestamp,
            STREAM_ID.to_owned(),
            payload,
        )?;
        if let Err(error) = self.journal.append_event(&envelope) {
            return self.poison(StorageError::Journal(error));
        }
        self.last_sequence = Some(sequence);
        if let Err(error) = self.journal.sync_events() {
            return self.poison(StorageError::Journal(error));
        }
        self.engine = preflight;
        result.map_err(StorageError::Risk)
    }

    /// Synchronizes the current durable prefix.
    ///
    /// # Errors
    ///
    /// A sync failure poisons this live owner.
    pub fn sync(&mut self) -> Result<(), StorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(StorageError::Halted(reason.clone()));
        }
        self.journal
            .sync_events()
            .map_err(StorageError::Journal)
            .or_else(|error| self.poison(error))
    }

    #[must_use]
    pub const fn engine(&self) -> &PortfolioRiskEngine {
        &self.engine
    }

    #[must_use]
    pub const fn last_sequence(&self) -> Option<u64> {
        self.last_sequence
    }

    fn poison<T>(&mut self, error: StorageError) -> Result<T, StorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Strictly replays a complete segmented risk-decision journal.
///
/// # Errors
///
/// Rejects corruption, gaps, invalid envelopes, domain inconsistency,
/// post-halt events, and checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    checkpoint: Option<RiskCheckpoint>,
) -> Result<RiskRecovery, StorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut engine = PortfolioRiskEngine::default();
    let mut expected = 0_u64;
    let mut last_sequence = None;
    let mut checkpoint_verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(envelope) = reader.next_event()? {
        if halted {
            return Err(StorageError::PostHaltEvent);
        }
        if envelope.sequence != expected {
            return Err(StorageError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(StorageError::SequenceExhausted)?;
        let command = validate_envelope(&envelope)?;
        if engine.apply(&command).is_err() {
            if !engine.is_halted() {
                return Err(StorageError::Risk(RiskError::Overflow));
            }
            halted = true;
        }
        last_sequence = Some(envelope.sequence);
        if checkpoint.is_some_and(|value| value.sequence == envelope.sequence) {
            let expected_digest = checkpoint
                .map(|value| value.risk_digest)
                .ok_or(StorageError::CheckpointMismatch)?;
            if engine.snapshot().digest != expected_digest {
                return Err(StorageError::CheckpointMismatch);
            }
            checkpoint_verified = true;
        }
    }
    if !checkpoint_verified {
        return Err(StorageError::CheckpointSequenceMissing);
    }
    Ok(RiskRecovery {
        engine,
        last_sequence,
    })
}

fn validate_envelope(envelope: &EventEnvelope) -> Result<RiskCommand, StorageError> {
    if envelope.source != EventSource::System || envelope.market_id != STREAM_ID {
        return Err(StorageError::EnvelopeIdentity);
    }
    let command = decode_command(&envelope.payload)?;
    if envelope.event_time_ns != command.recorded_at_ns()
        || envelope.received_time_ns != command.recorded_at_ns()
    {
        return Err(StorageError::EnvelopeTimestamp);
    }
    Ok(command)
}

/// Creates and device-syncs a new checkpoint without replacement.
///
/// # Errors
///
/// Returns I/O errors, including existing targets.
pub fn write_checkpoint_create_new(
    path: impl AsRef<Path>,
    checkpoint: RiskCheckpoint,
) -> Result<(), StorageError> {
    let bytes = encode_checkpoint(checkpoint);
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Reads and verifies one exact risk checkpoint.
///
/// # Errors
///
/// Rejects length, magic, version, reserved-byte, checksum, and I/O failures.
pub fn read_checkpoint(path: impl AsRef<Path>) -> Result<RiskCheckpoint, StorageError> {
    decode_checkpoint(&fs::read(path)?)
}

fn encode_checkpoint(checkpoint: RiskCheckpoint) -> [u8; CHECKPOINT_BYTES] {
    let mut bytes = [0_u8; CHECKPOINT_BYTES];
    bytes[0..8].copy_from_slice(CHECKPOINT_MAGIC);
    bytes[8..10].copy_from_slice(&CHECKPOINT_VERSION.to_le_bytes());
    bytes[16..24].copy_from_slice(&checkpoint.sequence.to_le_bytes());
    bytes[24..56].copy_from_slice(&checkpoint.risk_digest);
    let checksum = blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]);
    bytes[CHECKPOINT_BODY_BYTES..].copy_from_slice(checksum.as_bytes());
    bytes
}

fn decode_checkpoint(bytes: &[u8]) -> Result<RiskCheckpoint, StorageError> {
    if bytes.len() != CHECKPOINT_BYTES {
        return Err(StorageError::CheckpointLength);
    }
    if bytes.get(0..8) != Some(CHECKPOINT_MAGIC) {
        return Err(StorageError::CheckpointMagic);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| StorageError::CheckpointLength)?,
    );
    if version != CHECKPOINT_VERSION {
        return Err(StorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(StorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]).as_bytes() != &bytes[CHECKPOINT_BODY_BYTES..] {
        return Err(StorageError::CheckpointChecksum);
    }
    Ok(RiskCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| StorageError::CheckpointLength)?,
        ),
        risk_digest: bytes[24..56]
            .try_into()
            .map_err(|_| StorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("portfolio risk error: {0}")]
    Risk(#[from] RiskError),
    #[error("risk envelope error: {0:?}")]
    Schema(#[from] SchemaError),
    #[error("risk journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("risk segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("risk I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("risk sequence mismatch: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("risk sequence is exhausted")]
    SequenceExhausted,
    #[error("risk envelope source or identity is invalid")]
    EnvelopeIdentity,
    #[error("risk envelope timestamp does not match its command")]
    EnvelopeTimestamp,
    #[error("journal writer and recovered risk state disagree")]
    RecoveryMismatch,
    #[error("durable event follows an absorbing risk halt")]
    PostHaltEvent,
    #[error("risk checkpoint digest does not match its prefix")]
    CheckpointMismatch,
    #[error("risk checkpoint sequence is absent from the journal")]
    CheckpointSequenceMissing,
    #[error("risk checkpoint length is invalid")]
    CheckpointLength,
    #[error("risk checkpoint magic is invalid")]
    CheckpointMagic,
    #[error("unsupported risk checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("risk checkpoint reserved bytes are non-zero")]
    CheckpointReserved,
    #[error("risk checkpoint checksum mismatch")]
    CheckpointChecksum,
    #[error("durable risk engine is halted: {0}")]
    Halted(String),
}
