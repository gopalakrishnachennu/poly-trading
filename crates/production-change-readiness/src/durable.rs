use crate::{
    decode_command, encode_command, Error as ReadinessError, ProductionChangeReadiness,
    ProductionReadinessPolicy, ReadinessCommand, ReadinessOutcome,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

const STREAM_ID: &str = "__production_change_readiness__";
const CHECKPOINT_MAGIC: &[u8; 8] = b"POLYPRC1";
const CHECKPOINT_VERSION: u16 = 1;
const CHECKPOINT_BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReadinessCheckpoint {
    pub sequence: u64,
    pub readiness_digest: [u8; 32],
}

#[derive(Debug)]
pub struct ReadinessRecovery {
    pub readiness: ProductionChangeReadiness,
    pub last_sequence: Option<u64>,
}

#[derive(Debug)]
pub struct DurableProductionReadiness<J> {
    journal: J,
    readiness: ProductionChangeReadiness,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableProductionReadiness<J> {
    /// Aligns a journal writer with recovered readiness state.
    ///
    /// # Errors
    ///
    /// Rejects writer/recovery sequence disagreement.
    pub fn new(journal: J, recovery: ReadinessRecovery) -> Result<Self, ReadinessStorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(ReadinessStorageError::RecoveryMismatch);
        }
        Ok(Self {
            journal,
            readiness: recovery.readiness,
            last_sequence: recovery.last_sequence,
            poisoned: None,
        })
    }

    /// Journals and device-syncs before installing one transition.
    ///
    /// # Errors
    ///
    /// Returns state, schema, journal, sequence or poisoned-owner errors.
    pub fn apply(
        &mut self,
        command: &ReadinessCommand,
    ) -> Result<ReadinessOutcome, ReadinessStorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(ReadinessStorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut owner = self.readiness.clone();
        let result = owner.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |value| {
            value
                .checked_add(1)
                .ok_or(ReadinessStorageError::SequenceExhausted)
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
            return self.poison(ReadinessStorageError::Journal(error));
        }
        self.last_sequence = Some(sequence);
        if let Err(error) = self.journal.sync_events() {
            return self.poison(ReadinessStorageError::Journal(error));
        }
        self.readiness = owner;
        result.map_err(ReadinessStorageError::Readiness)
    }

    #[must_use]
    pub const fn readiness(&self) -> &ProductionChangeReadiness {
        &self.readiness
    }

    fn poison<T>(&mut self, error: ReadinessStorageError) -> Result<T, ReadinessStorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Strictly replays one segmented readiness journal.
///
/// # Errors
///
/// Rejects corruption, gaps, invalid envelopes, post-halt events and checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    policy: ProductionReadinessPolicy,
    checkpoint: Option<ReadinessCheckpoint>,
) -> Result<ReadinessRecovery, ReadinessStorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut owner = ProductionChangeReadiness::new(policy)?;
    let mut expected = 0_u64;
    let mut last_sequence = None;
    let mut checkpoint_verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(envelope) = reader.next_event()? {
        if halted {
            return Err(ReadinessStorageError::PostHaltEvent);
        }
        if envelope.sequence != expected {
            return Err(ReadinessStorageError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(ReadinessStorageError::SequenceExhausted)?;
        let command = validate_envelope(&envelope)?;
        if let Err(error) = owner.apply(&command) {
            if !owner.is_halted() {
                return Err(ReadinessStorageError::Readiness(error));
            }
            halted = true;
        }
        last_sequence = Some(envelope.sequence);
        if checkpoint.is_some_and(|value| value.sequence == envelope.sequence) {
            if owner.snapshot().digest
                != checkpoint
                    .map(|value| value.readiness_digest)
                    .ok_or(ReadinessStorageError::CheckpointMismatch)?
            {
                return Err(ReadinessStorageError::CheckpointMismatch);
            }
            checkpoint_verified = true;
        }
    }
    if !checkpoint_verified {
        return Err(ReadinessStorageError::CheckpointSequenceMissing);
    }
    Ok(ReadinessRecovery {
        readiness: owner,
        last_sequence,
    })
}

fn validate_envelope(envelope: &EventEnvelope) -> Result<ReadinessCommand, ReadinessStorageError> {
    if envelope.source != EventSource::System || envelope.market_id != STREAM_ID {
        return Err(ReadinessStorageError::EnvelopeIdentity);
    }
    let command = decode_command(&envelope.payload)?;
    if envelope.event_time_ns != command.recorded_at_ns()
        || envelope.received_time_ns != command.recorded_at_ns()
    {
        return Err(ReadinessStorageError::EnvelopeTimestamp);
    }
    Ok(command)
}

/// Creates and syncs one checkpoint without replacement.
///
/// # Errors
///
/// Returns I/O errors, including an existing target.
pub fn write_checkpoint_create_new(
    path: impl AsRef<Path>,
    checkpoint: ReadinessCheckpoint,
) -> Result<(), ReadinessStorageError> {
    let bytes = encode_checkpoint(checkpoint);
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Reads and verifies one checkpoint.
///
/// # Errors
///
/// Rejects length, magic, version, reserved bytes, checksum and I/O failures.
pub fn read_checkpoint(
    path: impl AsRef<Path>,
) -> Result<ReadinessCheckpoint, ReadinessStorageError> {
    decode_checkpoint(&fs::read(path)?)
}

fn encode_checkpoint(checkpoint: ReadinessCheckpoint) -> [u8; CHECKPOINT_BYTES] {
    let mut bytes = [0_u8; CHECKPOINT_BYTES];
    bytes[0..8].copy_from_slice(CHECKPOINT_MAGIC);
    bytes[8..10].copy_from_slice(&CHECKPOINT_VERSION.to_le_bytes());
    bytes[16..24].copy_from_slice(&checkpoint.sequence.to_le_bytes());
    bytes[24..56].copy_from_slice(&checkpoint.readiness_digest);
    let checksum = blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]);
    bytes[CHECKPOINT_BODY_BYTES..].copy_from_slice(checksum.as_bytes());
    bytes
}

fn decode_checkpoint(bytes: &[u8]) -> Result<ReadinessCheckpoint, ReadinessStorageError> {
    if bytes.len() != CHECKPOINT_BYTES {
        return Err(ReadinessStorageError::CheckpointLength);
    }
    if bytes.get(0..8) != Some(CHECKPOINT_MAGIC) {
        return Err(ReadinessStorageError::CheckpointMagic);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| ReadinessStorageError::CheckpointLength)?,
    );
    if version != CHECKPOINT_VERSION {
        return Err(ReadinessStorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(ReadinessStorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]).as_bytes() != &bytes[CHECKPOINT_BODY_BYTES..] {
        return Err(ReadinessStorageError::CheckpointChecksum);
    }
    Ok(ReadinessCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| ReadinessStorageError::CheckpointLength)?,
        ),
        readiness_digest: bytes[24..56]
            .try_into()
            .map_err(|_| ReadinessStorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum ReadinessStorageError {
    #[error("production readiness error: {0}")]
    Readiness(#[from] ReadinessError),
    #[error("production readiness envelope error: {0:?}")]
    Schema(#[from] SchemaError),
    #[error("production readiness journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("production readiness segmented journal error: {0}")]
    Segment(#[from] SegmentError),
    #[error("production readiness checkpoint I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("production readiness recovery sequence mismatch")]
    RecoveryMismatch,
    #[error("production readiness sequence exhausted")]
    SequenceExhausted,
    #[error("production readiness journal sequence mismatch: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("production readiness journal envelope identity is invalid")]
    EnvelopeIdentity,
    #[error("production readiness journal envelope timestamp is invalid")]
    EnvelopeTimestamp,
    #[error("production readiness checkpoint length is invalid")]
    CheckpointLength,
    #[error("production readiness checkpoint magic is invalid")]
    CheckpointMagic,
    #[error("unsupported production readiness checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("production readiness checkpoint reserved bytes are non-zero")]
    CheckpointReserved,
    #[error("production readiness checkpoint checksum is invalid")]
    CheckpointChecksum,
    #[error("production readiness checkpoint digest mismatch")]
    CheckpointMismatch,
    #[error("production readiness checkpoint sequence is missing")]
    CheckpointSequenceMissing,
    #[error("production readiness journal contains an event after halt")]
    PostHaltEvent,
    #[error("durable production readiness is halted: {0}")]
    Halted(String),
}
