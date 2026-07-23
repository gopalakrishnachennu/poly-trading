use crate::{
    decode_command, encode_command, Error as SessionError, ExecutorSessionPolicy,
    ExecutorSessionSimulator, SessionCommand, SessionOutcome,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};
use thiserror::Error;

const STREAM: &str = "__executor_session_simulator__";
const MAGIC: &[u8; 8] = b"POLYESC1";
const VERSION: u16 = 1;
const BODY: usize = 56;
const BYTES: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ExecutorSessionCheckpoint {
    pub sequence: u64,
    pub state_digest: [u8; 32],
}
#[derive(Debug)]
pub struct ExecutorSessionRecovery {
    pub owner: ExecutorSessionSimulator,
    pub last_sequence: Option<u64>,
}
#[derive(Debug)]
pub struct DurableExecutorSession<J> {
    journal: J,
    owner: ExecutorSessionSimulator,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableExecutorSession<J> {
    /// Aligns a journal writer with recovered session state.
    ///
    /// # Errors
    ///
    /// Rejects writer and recovery sequence disagreement.
    pub fn new(
        journal: J,
        recovery: ExecutorSessionRecovery,
    ) -> Result<Self, ExecutorSessionStorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(ExecutorSessionStorageError::RecoveryMismatch);
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
    /// Returns state, envelope, journal, sequence, or poisoned-owner failures.
    pub fn apply(
        &mut self,
        command: &SessionCommand,
    ) -> Result<SessionOutcome, ExecutorSessionStorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(ExecutorSessionStorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut next = self.owner.clone();
        let result = next.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |v| {
            v.checked_add(1)
                .ok_or(ExecutorSessionStorageError::SequenceExhausted)
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
        if let Err(e) = self.journal.append_event(&envelope) {
            return self.poison(ExecutorSessionStorageError::Journal(e));
        }
        self.last_sequence = Some(sequence);
        if let Err(e) = self.journal.sync_events() {
            return self.poison(ExecutorSessionStorageError::Journal(e));
        }
        self.owner = next;
        result.map_err(ExecutorSessionStorageError::Session)
    }
    #[must_use]
    pub const fn owner(&self) -> &ExecutorSessionSimulator {
        &self.owner
    }
    fn poison<T>(
        &mut self,
        error: ExecutorSessionStorageError,
    ) -> Result<T, ExecutorSessionStorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Strictly replays one segmented session journal.
///
/// # Errors
///
/// Rejects corruption, gaps, invalid envelopes, post-halt events, and checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    policy: ExecutorSessionPolicy,
    checkpoint: Option<ExecutorSessionCheckpoint>,
) -> Result<ExecutorSessionRecovery, ExecutorSessionStorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut owner = ExecutorSessionSimulator::new(policy)?;
    let mut expected = 0;
    let mut last = None;
    let mut verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(envelope) = reader.next_event()? {
        if halted {
            return Err(ExecutorSessionStorageError::PostHaltEvent);
        }
        if envelope.sequence != expected {
            return Err(ExecutorSessionStorageError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(ExecutorSessionStorageError::SequenceExhausted)?;
        if envelope.source != EventSource::System || envelope.market_id != STREAM {
            return Err(ExecutorSessionStorageError::EnvelopeIdentity);
        }
        let command = decode_command(&envelope.payload)?;
        if envelope.event_time_ns != command.recorded_at_ns()
            || envelope.received_time_ns != command.recorded_at_ns()
        {
            return Err(ExecutorSessionStorageError::EnvelopeTimestamp);
        }
        if owner.apply(&command).is_err() {
            if !owner.is_halted() {
                return Err(ExecutorSessionStorageError::Replay);
            }
            halted = true;
        }
        last = Some(envelope.sequence);
        if checkpoint.is_some_and(|v| v.sequence == envelope.sequence) {
            if owner.snapshot().digest
                != checkpoint
                    .ok_or(ExecutorSessionStorageError::CheckpointMismatch)?
                    .state_digest
            {
                return Err(ExecutorSessionStorageError::CheckpointMismatch);
            }
            verified = true;
        }
    }
    if !verified {
        return Err(ExecutorSessionStorageError::CheckpointSequenceMissing);
    }
    Ok(ExecutorSessionRecovery {
        owner,
        last_sequence: last,
    })
}

/// Creates and syncs one checkpoint without replacement.
///
/// # Errors
///
/// Returns I/O failures, including an existing target.
pub fn write_checkpoint_create_new(
    path: impl AsRef<Path>,
    checkpoint: ExecutorSessionCheckpoint,
) -> Result<(), ExecutorSessionStorageError> {
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
) -> Result<ExecutorSessionCheckpoint, ExecutorSessionStorageError> {
    let bytes = fs::read(path)?;
    if bytes.len() != BYTES || bytes.get(..8) != Some(MAGIC) {
        return Err(ExecutorSessionStorageError::CheckpointLength);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| ExecutorSessionStorageError::CheckpointLength)?,
    );
    if version != VERSION {
        return Err(ExecutorSessionStorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(ExecutorSessionStorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..BODY]).as_bytes() != &bytes[BODY..] {
        return Err(ExecutorSessionStorageError::CheckpointChecksum);
    }
    Ok(ExecutorSessionCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| ExecutorSessionStorageError::CheckpointLength)?,
        ),
        state_digest: bytes[24..56]
            .try_into()
            .map_err(|_| ExecutorSessionStorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum ExecutorSessionStorageError {
    #[error("executor session error: {0}")]
    Session(#[from] SessionError),
    #[error("executor session envelope error: {0:?}")]
    Schema(#[from] SchemaError),
    #[error("executor session journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("executor session segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("executor session checkpoint I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("executor session recovery mismatch")]
    RecoveryMismatch,
    #[error("executor session sequence expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("executor session sequence exhausted")]
    SequenceExhausted,
    #[error("executor session envelope identity invalid")]
    EnvelopeIdentity,
    #[error("executor session envelope timestamp invalid")]
    EnvelopeTimestamp,
    #[error("executor session replay failed without halt")]
    Replay,
    #[error("executor session post-halt event exists")]
    PostHaltEvent,
    #[error("executor session checkpoint length invalid")]
    CheckpointLength,
    #[error("unsupported executor session checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("executor session checkpoint reserved bytes are non-zero")]
    CheckpointReserved,
    #[error("executor session checkpoint checksum invalid")]
    CheckpointChecksum,
    #[error("executor session checkpoint mismatch")]
    CheckpointMismatch,
    #[error("executor session checkpoint sequence missing")]
    CheckpointSequenceMissing,
    #[error("executor session durable owner halted: {0}")]
    Halted(String),
}
