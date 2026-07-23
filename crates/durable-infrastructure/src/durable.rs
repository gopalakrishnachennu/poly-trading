use crate::{
    decode_command, encode_command, DurableInfrastructureCertification, Error as CoreError,
    InfrastructureCommand, InfrastructureOutcome, InfrastructurePolicy,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::{fs, fs::OpenOptions, io::Write, path::Path};
use thiserror::Error;

const STREAM: &str = "__durable_infrastructure__";
const MAGIC: &[u8; 8] = b"POLYDIC1";
const VERSION: u16 = 1;
const BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InfrastructureCheckpoint {
    pub sequence: u64,
    pub state_digest: [u8; 32],
}

#[derive(Debug)]
pub struct InfrastructureRecovery {
    pub owner: DurableInfrastructureCertification,
    pub last_sequence: Option<u64>,
}

#[derive(Debug)]
pub struct DurableInfrastructureOwner<J> {
    journal: J,
    owner: DurableInfrastructureCertification,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableInfrastructureOwner<J> {
    /// Aligns a writer with recovered state.
    ///
    /// # Errors
    ///
    /// Rejects journal and recovered sequence disagreement.
    pub fn new(
        journal: J,
        recovery: InfrastructureRecovery,
    ) -> Result<Self, InfrastructureStorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(InfrastructureStorageError::RecoveryMismatch);
        }
        Ok(Self {
            journal,
            owner: recovery.owner,
            last_sequence: recovery.last_sequence,
            poisoned: None,
        })
    }

    /// Journals and syncs before installing one transition.
    ///
    /// # Errors
    ///
    /// Returns state, encoding, journal, sequence, or poisoned-owner failures.
    pub fn apply(
        &mut self,
        command: &InfrastructureCommand,
    ) -> Result<InfrastructureOutcome, InfrastructureStorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(InfrastructureStorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut next = self.owner.clone();
        let result = next.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |prior| {
            prior
                .checked_add(1)
                .ok_or(InfrastructureStorageError::SequenceExhausted)
        })?;
        let at = command.recorded_at_ns();
        let envelope = EventEnvelope::new(
            EventSource::System,
            sequence,
            at,
            at,
            STREAM.to_owned(),
            payload,
        )?;
        if let Err(error) = self.journal.append_event(&envelope) {
            return self.poison(InfrastructureStorageError::Journal(error));
        }
        self.last_sequence = Some(sequence);
        if let Err(error) = self.journal.sync_events() {
            return self.poison(InfrastructureStorageError::Journal(error));
        }
        self.owner = next;
        result.map_err(InfrastructureStorageError::Core)
    }
    #[must_use]
    pub const fn owner(&self) -> &DurableInfrastructureCertification {
        &self.owner
    }
    fn poison<T>(
        &mut self,
        error: InfrastructureStorageError,
    ) -> Result<T, InfrastructureStorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Strictly replays one segmented infrastructure journal.
///
/// # Errors
///
/// Rejects corruption, gaps, invalid envelopes, post-halt events and checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    policy: InfrastructurePolicy,
    checkpoint: Option<InfrastructureCheckpoint>,
) -> Result<InfrastructureRecovery, InfrastructureStorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut owner = DurableInfrastructureCertification::new(policy)?;
    let mut expected = 0;
    let mut last = None;
    let mut verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(envelope) = reader.next_event()? {
        if halted {
            return Err(InfrastructureStorageError::PostHaltEvent);
        }
        if envelope.sequence != expected {
            return Err(InfrastructureStorageError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(InfrastructureStorageError::SequenceExhausted)?;
        if envelope.source != EventSource::System || envelope.market_id != STREAM {
            return Err(InfrastructureStorageError::EnvelopeIdentity);
        }
        let command = decode_command(&envelope.payload)?;
        if envelope.event_time_ns != command.recorded_at_ns()
            || envelope.received_time_ns != command.recorded_at_ns()
        {
            return Err(InfrastructureStorageError::EnvelopeTimestamp);
        }
        if owner.apply(&command).is_err() {
            if !owner.is_halted() {
                return Err(InfrastructureStorageError::Replay);
            }
            halted = true;
        }
        last = Some(envelope.sequence);
        if checkpoint.is_some_and(|value| value.sequence == envelope.sequence) {
            if owner.snapshot().digest
                != checkpoint
                    .ok_or(InfrastructureStorageError::CheckpointMismatch)?
                    .state_digest
            {
                return Err(InfrastructureStorageError::CheckpointMismatch);
            }
            verified = true;
        }
    }
    if !verified {
        return Err(InfrastructureStorageError::CheckpointSequenceMissing);
    }
    Ok(InfrastructureRecovery {
        owner,
        last_sequence: last,
    })
}

/// Creates and syncs one checkpoint without replacement.
///
/// # Errors
///
/// Returns I/O errors, including an existing target.
pub fn write_checkpoint_create_new(
    path: impl AsRef<Path>,
    checkpoint: InfrastructureCheckpoint,
) -> Result<(), InfrastructureStorageError> {
    let mut bytes = [0_u8; CHECKPOINT_BYTES];
    bytes[..8].copy_from_slice(MAGIC);
    bytes[8..10].copy_from_slice(&VERSION.to_le_bytes());
    bytes[16..24].copy_from_slice(&checkpoint.sequence.to_le_bytes());
    bytes[24..56].copy_from_slice(&checkpoint.state_digest);
    let checksum = blake3::hash(&bytes[..BODY_BYTES]);
    bytes[BODY_BYTES..].copy_from_slice(checksum.as_bytes());
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
) -> Result<InfrastructureCheckpoint, InfrastructureStorageError> {
    let bytes = fs::read(path)?;
    if bytes.len() != CHECKPOINT_BYTES || bytes.get(..8) != Some(MAGIC) {
        return Err(InfrastructureStorageError::CheckpointLength);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| InfrastructureStorageError::CheckpointLength)?,
    );
    if version != VERSION {
        return Err(InfrastructureStorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(InfrastructureStorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..BODY_BYTES]).as_bytes() != &bytes[BODY_BYTES..] {
        return Err(InfrastructureStorageError::CheckpointChecksum);
    }
    Ok(InfrastructureCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| InfrastructureStorageError::CheckpointLength)?,
        ),
        state_digest: bytes[24..56]
            .try_into()
            .map_err(|_| InfrastructureStorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum InfrastructureStorageError {
    #[error("infrastructure core error: {0}")]
    Core(#[from] CoreError),
    #[error("infrastructure schema error: {0}")]
    Schema(#[from] SchemaError),
    #[error("infrastructure journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("infrastructure segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("infrastructure I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("infrastructure recovery/writer mismatch")]
    RecoveryMismatch,
    #[error("infrastructure sequence exhausted")]
    SequenceExhausted,
    #[error("infrastructure sequence mismatch: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("infrastructure envelope identity invalid")]
    EnvelopeIdentity,
    #[error("infrastructure envelope timestamp invalid")]
    EnvelopeTimestamp,
    #[error("infrastructure replay rejected transition")]
    Replay,
    #[error("infrastructure event follows halt")]
    PostHaltEvent,
    #[error("infrastructure checkpoint length or magic invalid")]
    CheckpointLength,
    #[error("unsupported infrastructure checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("infrastructure checkpoint reserved bytes non-zero")]
    CheckpointReserved,
    #[error("infrastructure checkpoint checksum invalid")]
    CheckpointChecksum,
    #[error("infrastructure checkpoint state digest mismatch")]
    CheckpointMismatch,
    #[error("infrastructure checkpoint sequence absent from journal")]
    CheckpointSequenceMissing,
    #[error("durable infrastructure owner halted: {0}")]
    Halted(String),
}
