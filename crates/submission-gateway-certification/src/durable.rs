use crate::{
    decode_command, encode_command, Error as GatewayError, GatewayCommand, GatewayOutcome,
    GatewayPolicy, SubmissionGatewayCertification,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};
use thiserror::Error;

const STREAM: &str = "__submission_gateway_certification__";
const MAGIC: &[u8; 8] = b"POLYSGC1";
const VERSION: u16 = 1;
const BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GatewayCheckpoint {
    pub sequence: u64,
    pub state_digest: [u8; 32],
}

#[derive(Debug)]
pub struct GatewayRecovery {
    pub owner: SubmissionGatewayCertification,
    pub last_sequence: Option<u64>,
}

#[derive(Debug)]
pub struct DurableSubmissionGateway<J> {
    journal: J,
    owner: SubmissionGatewayCertification,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableSubmissionGateway<J> {
    /// Aligns a journal writer with recovered state.
    ///
    /// # Errors
    ///
    /// Rejects writer and recovery sequence disagreement.
    pub fn new(journal: J, recovery: GatewayRecovery) -> Result<Self, GatewayStorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(GatewayStorageError::RecoveryMismatch);
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
    /// Returns state, schema, journal, sequence, or poisoned-owner failures.
    pub fn apply(
        &mut self,
        command: &GatewayCommand,
    ) -> Result<GatewayOutcome, GatewayStorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(GatewayStorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut next = self.owner.clone();
        let result = next.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |prior| {
            prior
                .checked_add(1)
                .ok_or(GatewayStorageError::SequenceExhausted)
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
            return self.poison(GatewayStorageError::Journal(error));
        }
        self.last_sequence = Some(sequence);
        if let Err(error) = self.journal.sync_events() {
            return self.poison(GatewayStorageError::Journal(error));
        }
        self.owner = next;
        result.map_err(GatewayStorageError::Gateway)
    }

    #[must_use]
    pub const fn owner(&self) -> &SubmissionGatewayCertification {
        &self.owner
    }

    fn poison<T>(&mut self, error: GatewayStorageError) -> Result<T, GatewayStorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Strictly replays one segmented gateway journal.
///
/// # Errors
///
/// Rejects corruption, gaps, invalid envelopes, post-halt events, and checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    policy: GatewayPolicy,
    checkpoint: Option<GatewayCheckpoint>,
) -> Result<GatewayRecovery, GatewayStorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut owner = SubmissionGatewayCertification::new(policy)?;
    let mut expected = 0;
    let mut last = None;
    let mut checkpoint_verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(envelope) = reader.next_event()? {
        if halted {
            return Err(GatewayStorageError::PostHaltEvent);
        }
        if envelope.sequence != expected {
            return Err(GatewayStorageError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(GatewayStorageError::SequenceExhausted)?;
        if envelope.source != EventSource::System || envelope.market_id != STREAM {
            return Err(GatewayStorageError::EnvelopeIdentity);
        }
        let command = decode_command(&envelope.payload)?;
        if envelope.event_time_ns != command.recorded_at_ns()
            || envelope.received_time_ns != command.recorded_at_ns()
        {
            return Err(GatewayStorageError::EnvelopeTimestamp);
        }
        if owner.apply(&command).is_err() {
            if !owner.is_halted() {
                return Err(GatewayStorageError::Replay);
            }
            halted = true;
        }
        last = Some(envelope.sequence);
        if checkpoint.is_some_and(|value| value.sequence == envelope.sequence) {
            if owner.snapshot().digest
                != checkpoint
                    .ok_or(GatewayStorageError::CheckpointMismatch)?
                    .state_digest
            {
                return Err(GatewayStorageError::CheckpointMismatch);
            }
            checkpoint_verified = true;
        }
    }
    if !checkpoint_verified {
        return Err(GatewayStorageError::CheckpointSequenceMissing);
    }
    Ok(GatewayRecovery {
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
    checkpoint: GatewayCheckpoint,
) -> Result<(), GatewayStorageError> {
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
/// Rejects length, magic, version, reserved bytes, checksum, and I/O failures.
pub fn read_checkpoint(path: impl AsRef<Path>) -> Result<GatewayCheckpoint, GatewayStorageError> {
    let bytes = fs::read(path)?;
    if bytes.len() != CHECKPOINT_BYTES || bytes.get(..8) != Some(MAGIC) {
        return Err(GatewayStorageError::CheckpointLength);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| GatewayStorageError::CheckpointLength)?,
    );
    if version != VERSION {
        return Err(GatewayStorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(GatewayStorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..BODY_BYTES]).as_bytes() != &bytes[BODY_BYTES..] {
        return Err(GatewayStorageError::CheckpointChecksum);
    }
    Ok(GatewayCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| GatewayStorageError::CheckpointLength)?,
        ),
        state_digest: bytes[24..56]
            .try_into()
            .map_err(|_| GatewayStorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum GatewayStorageError {
    #[error("submission gateway error: {0}")]
    Gateway(#[from] GatewayError),
    #[error("submission gateway envelope error: {0:?}")]
    Schema(#[from] SchemaError),
    #[error("submission gateway journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("submission gateway segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("submission gateway checkpoint I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("submission gateway recovery mismatch")]
    RecoveryMismatch,
    #[error("submission gateway sequence expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("submission gateway sequence exhausted")]
    SequenceExhausted,
    #[error("submission gateway envelope identity invalid")]
    EnvelopeIdentity,
    #[error("submission gateway envelope timestamp invalid")]
    EnvelopeTimestamp,
    #[error("submission gateway replay failed without halt")]
    Replay,
    #[error("submission gateway post-halt event exists")]
    PostHaltEvent,
    #[error("submission gateway checkpoint length invalid")]
    CheckpointLength,
    #[error("unsupported submission gateway checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("submission gateway checkpoint reserved bytes non-zero")]
    CheckpointReserved,
    #[error("submission gateway checkpoint checksum invalid")]
    CheckpointChecksum,
    #[error("submission gateway checkpoint mismatch")]
    CheckpointMismatch,
    #[error("submission gateway checkpoint sequence missing")]
    CheckpointSequenceMissing,
    #[error("submission gateway durable owner halted: {0}")]
    Halted(String),
}
