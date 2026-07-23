use crate::{
    decode_command, encode_command, Error as ProposalError, ProposalCommand, ProposalDecision,
    ProposalEngine,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

const STREAM_ID: &str = "__strategy_proposal__";
const CHECKPOINT_MAGIC: &[u8; 8] = b"POLYSPP1";
const CHECKPOINT_VERSION: u16 = 1;
const CHECKPOINT_BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ProposalCheckpoint {
    pub sequence: u64,
    pub engine_digest: [u8; 32],
}

#[derive(Debug)]
pub struct ProposalRecovery {
    pub engine: ProposalEngine,
    pub last_sequence: Option<u64>,
}

#[derive(Debug)]
pub struct DurableProposalEngine<J> {
    journal: J,
    engine: ProposalEngine,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableProposalEngine<J> {
    /// Aligns a writer with recovered proposal state.
    ///
    /// # Errors
    ///
    /// Rejects writer/recovery sequence disagreement.
    pub fn new(journal: J, recovery: ProposalRecovery) -> Result<Self, StorageError> {
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

    /// Appends and syncs before installing one proposal transition.
    ///
    /// # Errors
    ///
    /// Returns domain, schema, journal, sequence, or poisoned-owner failures.
    pub fn apply(&mut self, command: &ProposalCommand) -> Result<ProposalDecision, StorageError> {
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
        result.map_err(StorageError::Proposal)
    }

    #[must_use]
    pub const fn engine(&self) -> &ProposalEngine {
        &self.engine
    }

    fn poison<T>(&mut self, error: StorageError) -> Result<T, StorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Strictly replays a segmented proposal journal.
///
/// # Errors
///
/// Rejects corruption, gaps, invalid envelopes, post-halt events, and checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    checkpoint: Option<ProposalCheckpoint>,
) -> Result<ProposalRecovery, StorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut engine = ProposalEngine::default();
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
                return Err(StorageError::Proposal(ProposalError::ContextHistory));
            }
            halted = true;
        }
        last_sequence = Some(envelope.sequence);
        if checkpoint.is_some_and(|value| value.sequence == envelope.sequence) {
            if engine.snapshot().digest
                != checkpoint
                    .map(|value| value.engine_digest)
                    .ok_or(StorageError::CheckpointMismatch)?
            {
                return Err(StorageError::CheckpointMismatch);
            }
            checkpoint_verified = true;
        }
    }
    if !checkpoint_verified {
        return Err(StorageError::CheckpointSequenceMissing);
    }
    Ok(ProposalRecovery {
        engine,
        last_sequence,
    })
}

fn validate_envelope(envelope: &EventEnvelope) -> Result<ProposalCommand, StorageError> {
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

/// Creates and syncs a new proposal checkpoint without replacement.
///
/// # Errors
///
/// Returns I/O errors, including an existing target.
pub fn write_checkpoint_create_new(
    path: impl AsRef<Path>,
    checkpoint: ProposalCheckpoint,
) -> Result<(), StorageError> {
    let bytes = encode_checkpoint(checkpoint);
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Reads and verifies one exact proposal checkpoint.
///
/// # Errors
///
/// Rejects length, magic, version, reserved bytes, checksum, and I/O failures.
pub fn read_checkpoint(path: impl AsRef<Path>) -> Result<ProposalCheckpoint, StorageError> {
    decode_checkpoint(&fs::read(path)?)
}

fn encode_checkpoint(checkpoint: ProposalCheckpoint) -> [u8; CHECKPOINT_BYTES] {
    let mut bytes = [0_u8; CHECKPOINT_BYTES];
    bytes[0..8].copy_from_slice(CHECKPOINT_MAGIC);
    bytes[8..10].copy_from_slice(&CHECKPOINT_VERSION.to_le_bytes());
    bytes[16..24].copy_from_slice(&checkpoint.sequence.to_le_bytes());
    bytes[24..56].copy_from_slice(&checkpoint.engine_digest);
    let checksum = blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]);
    bytes[CHECKPOINT_BODY_BYTES..].copy_from_slice(checksum.as_bytes());
    bytes
}

fn decode_checkpoint(bytes: &[u8]) -> Result<ProposalCheckpoint, StorageError> {
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
    Ok(ProposalCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| StorageError::CheckpointLength)?,
        ),
        engine_digest: bytes[24..56]
            .try_into()
            .map_err(|_| StorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("strategy proposal error: {0}")]
    Proposal(#[from] ProposalError),
    #[error("proposal envelope error: {0:?}")]
    Schema(#[from] SchemaError),
    #[error("proposal journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("proposal segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("proposal I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("proposal sequence mismatch: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("proposal sequence is exhausted")]
    SequenceExhausted,
    #[error("proposal envelope identity is invalid")]
    EnvelopeIdentity,
    #[error("proposal envelope timestamp does not match command")]
    EnvelopeTimestamp,
    #[error("proposal writer and recovered engine disagree")]
    RecoveryMismatch,
    #[error("durable proposal follows an absorbing halt")]
    PostHaltEvent,
    #[error("proposal checkpoint digest does not match its prefix")]
    CheckpointMismatch,
    #[error("proposal checkpoint sequence is absent from journal")]
    CheckpointSequenceMissing,
    #[error("proposal checkpoint length is invalid")]
    CheckpointLength,
    #[error("proposal checkpoint magic is invalid")]
    CheckpointMagic,
    #[error("unsupported proposal checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("proposal checkpoint reserved bytes are non-zero")]
    CheckpointReserved,
    #[error("proposal checkpoint checksum mismatch")]
    CheckpointChecksum,
    #[error("durable proposal engine is halted: {0}")]
    Halted(String),
}
