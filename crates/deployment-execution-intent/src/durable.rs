use crate::{
    decode_command, encode_command, DeploymentExecutionIntent, Error as IntentError,
    ExecutionCommand, ExecutionIntentPolicy, ExecutionOutcome,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};
use thiserror::Error;

const STREAM: &str = "__deployment_execution_intent__";
const MAGIC: &[u8; 8] = b"POLYEIC1";
const VERSION: u16 = 1;
const BODY: usize = 56;
const BYTES: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutionCheckpoint {
    pub sequence: u64,
    pub state_digest: [u8; 32],
}

#[derive(Debug)]
pub struct ExecutionRecovery {
    pub owner: DeploymentExecutionIntent,
    pub last_sequence: Option<u64>,
}

#[derive(Debug)]
pub struct DurableExecutionIntent<J> {
    journal: J,
    owner: DeploymentExecutionIntent,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableExecutionIntent<J> {
    /// Aligns a journal writer with a recovered owner.
    ///
    /// # Errors
    ///
    /// Rejects journal and recovery sequence disagreement.
    pub fn new(journal: J, recovery: ExecutionRecovery) -> Result<Self, ExecutionStorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(ExecutionStorageError::RecoveryMismatch);
        }
        Ok(Self {
            journal,
            owner: recovery.owner,
            last_sequence: recovery.last_sequence,
            poisoned: None,
        })
    }
    /// Journals and device-syncs before installing one transition.
    ///
    /// # Errors
    ///
    /// Returns state, schema, journal, sequence, or poisoned-owner failures.
    pub fn apply(
        &mut self,
        command: &ExecutionCommand,
    ) -> Result<ExecutionOutcome, ExecutionStorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(ExecutionStorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut next = self.owner.clone();
        let result = next.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |v| {
            v.checked_add(1)
                .ok_or(ExecutionStorageError::SequenceExhausted)
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
            return self.poison(ExecutionStorageError::Journal(error));
        }
        self.last_sequence = Some(sequence);
        if let Err(error) = self.journal.sync_events() {
            return self.poison(ExecutionStorageError::Journal(error));
        }
        self.owner = next;
        result.map_err(ExecutionStorageError::Intent)
    }
    #[must_use]
    pub const fn owner(&self) -> &DeploymentExecutionIntent {
        &self.owner
    }
    fn poison<T>(&mut self, error: ExecutionStorageError) -> Result<T, ExecutionStorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Strictly replays one segmented execution-intent journal.
///
/// # Errors
///
/// Rejects corruption, gaps, invalid envelopes, post-halt events, and checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    policy: ExecutionIntentPolicy,
    checkpoint: Option<ExecutionCheckpoint>,
) -> Result<ExecutionRecovery, ExecutionStorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut owner = DeploymentExecutionIntent::new(policy)?;
    let mut expected = 0;
    let mut last = None;
    let mut verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(envelope) = reader.next_event()? {
        if halted {
            return Err(ExecutionStorageError::PostHaltEvent);
        }
        if envelope.sequence != expected {
            return Err(ExecutionStorageError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(ExecutionStorageError::SequenceExhausted)?;
        if envelope.source != EventSource::System || envelope.market_id != STREAM {
            return Err(ExecutionStorageError::EnvelopeIdentity);
        }
        let command = decode_command(&envelope.payload)?;
        if envelope.event_time_ns != command.recorded_at_ns()
            || envelope.received_time_ns != command.recorded_at_ns()
        {
            return Err(ExecutionStorageError::EnvelopeTimestamp);
        }
        if owner.apply(&command).is_err() {
            if !owner.is_halted() {
                return Err(ExecutionStorageError::Replay);
            }
            halted = true;
        }
        last = Some(envelope.sequence);
        if checkpoint.is_some_and(|v| v.sequence == envelope.sequence) {
            if owner.snapshot().digest
                != checkpoint
                    .ok_or(ExecutionStorageError::CheckpointMismatch)?
                    .state_digest
            {
                return Err(ExecutionStorageError::CheckpointMismatch);
            }
            verified = true;
        }
    }
    if !verified {
        return Err(ExecutionStorageError::CheckpointSequenceMissing);
    }
    Ok(ExecutionRecovery {
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
    checkpoint: ExecutionCheckpoint,
) -> Result<(), ExecutionStorageError> {
    let mut bytes = [0_u8; BYTES];
    bytes[..8].copy_from_slice(MAGIC);
    bytes[8..10].copy_from_slice(&VERSION.to_le_bytes());
    bytes[16..24].copy_from_slice(&checkpoint.sequence.to_le_bytes());
    bytes[24..56].copy_from_slice(&checkpoint.state_digest);
    let sum = blake3::hash(&bytes[..BODY]);
    bytes[BODY..].copy_from_slice(sum.as_bytes());
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
pub fn read_checkpoint(
    path: impl AsRef<Path>,
) -> Result<ExecutionCheckpoint, ExecutionStorageError> {
    let bytes = fs::read(path)?;
    if bytes.len() != BYTES || bytes.get(..8) != Some(MAGIC) {
        return Err(ExecutionStorageError::CheckpointLength);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| ExecutionStorageError::CheckpointLength)?,
    );
    if version != VERSION {
        return Err(ExecutionStorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(ExecutionStorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..BODY]).as_bytes() != &bytes[BODY..] {
        return Err(ExecutionStorageError::CheckpointChecksum);
    }
    Ok(ExecutionCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| ExecutionStorageError::CheckpointLength)?,
        ),
        state_digest: bytes[24..56]
            .try_into()
            .map_err(|_| ExecutionStorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum ExecutionStorageError {
    #[error("execution intent error: {0}")]
    Intent(#[from] IntentError),
    #[error("execution envelope error: {0:?}")]
    Schema(#[from] SchemaError),
    #[error("execution journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("execution segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("execution checkpoint I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("execution recovery mismatch")]
    RecoveryMismatch,
    #[error("execution sequence expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("execution sequence exhausted")]
    SequenceExhausted,
    #[error("execution envelope identity invalid")]
    EnvelopeIdentity,
    #[error("execution envelope timestamp invalid")]
    EnvelopeTimestamp,
    #[error("execution replay failed without halt")]
    Replay,
    #[error("execution post-halt event exists")]
    PostHaltEvent,
    #[error("execution checkpoint length invalid")]
    CheckpointLength,
    #[error("execution checkpoint version unsupported: {0}")]
    CheckpointVersion(u16),
    #[error("execution checkpoint reserved bytes invalid")]
    CheckpointReserved,
    #[error("execution checkpoint checksum invalid")]
    CheckpointChecksum,
    #[error("execution checkpoint mismatch")]
    CheckpointMismatch,
    #[error("execution checkpoint sequence missing")]
    CheckpointSequenceMissing,
    #[error("execution durable owner halted: {0}")]
    Halted(String),
}
