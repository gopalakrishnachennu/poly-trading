use crate::{
    decode_command, encode_command, Error as SessionError, SessionCommand, SessionOutcome,
    SessionPolicy, ShadowAuthSessionCoordinator,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};
use thiserror::Error;

const STREAM: &str = "__shadow_auth_session__";
const MAGIC: &[u8; 8] = b"POLYSAC1";
const VERSION: u16 = 1;
const BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SessionCheckpoint {
    pub sequence: u64,
    pub state_digest: [u8; 32],
}

#[derive(Debug)]
pub struct SessionRecovery {
    pub owner: ShadowAuthSessionCoordinator,
    pub last_sequence: Option<u64>,
}

#[derive(Debug)]
pub struct DurableShadowAuthSession<J> {
    journal: J,
    owner: ShadowAuthSessionCoordinator,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableShadowAuthSession<J> {
    /// Aligns a writer with recovered state.
    ///
    /// # Errors
    ///
    /// Rejects sequence disagreement.
    pub fn new(journal: J, recovery: SessionRecovery) -> Result<Self, SessionStorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(SessionStorageError::RecoveryMismatch);
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
        command: &SessionCommand,
    ) -> Result<SessionOutcome, SessionStorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(SessionStorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut next = self.owner.clone();
        let result = next.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |prior| {
            prior
                .checked_add(1)
                .ok_or(SessionStorageError::SequenceExhausted)
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
            return self.poison(SessionStorageError::Journal(error));
        }
        self.last_sequence = Some(sequence);
        if let Err(error) = self.journal.sync_events() {
            return self.poison(SessionStorageError::Journal(error));
        }
        self.owner = next;
        result.map_err(SessionStorageError::Session)
    }

    #[must_use]
    pub const fn owner(&self) -> &ShadowAuthSessionCoordinator {
        &self.owner
    }

    fn poison<T>(&mut self, error: SessionStorageError) -> Result<T, SessionStorageError> {
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
    policy: SessionPolicy,
    checkpoint: Option<SessionCheckpoint>,
) -> Result<SessionRecovery, SessionStorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut owner = ShadowAuthSessionCoordinator::new(policy)?;
    let mut expected = 0;
    let mut last = None;
    let mut verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(envelope) = reader.next_event()? {
        if halted {
            return Err(SessionStorageError::PostHaltEvent);
        }
        if envelope.sequence != expected {
            return Err(SessionStorageError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(SessionStorageError::SequenceExhausted)?;
        if envelope.source != EventSource::System || envelope.market_id != STREAM {
            return Err(SessionStorageError::EnvelopeIdentity);
        }
        let command = decode_command(&envelope.payload)?;
        if envelope.event_time_ns != command.recorded_at_ns()
            || envelope.received_time_ns != command.recorded_at_ns()
        {
            return Err(SessionStorageError::EnvelopeTimestamp);
        }
        if owner.apply(&command).is_err() {
            if !owner.is_halted() {
                return Err(SessionStorageError::Replay);
            }
            halted = true;
        }
        last = Some(envelope.sequence);
        if checkpoint.is_some_and(|value| value.sequence == envelope.sequence) {
            if owner.snapshot().digest
                != checkpoint
                    .ok_or(SessionStorageError::CheckpointMismatch)?
                    .state_digest
            {
                return Err(SessionStorageError::CheckpointMismatch);
            }
            verified = true;
        }
    }
    if !verified {
        return Err(SessionStorageError::CheckpointSequenceMissing);
    }
    Ok(SessionRecovery {
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
    checkpoint: SessionCheckpoint,
) -> Result<(), SessionStorageError> {
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
pub fn read_checkpoint(path: impl AsRef<Path>) -> Result<SessionCheckpoint, SessionStorageError> {
    let bytes = fs::read(path)?;
    if bytes.len() != CHECKPOINT_BYTES || bytes.get(..8) != Some(MAGIC) {
        return Err(SessionStorageError::CheckpointLength);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| SessionStorageError::CheckpointLength)?,
    );
    if version != VERSION {
        return Err(SessionStorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(SessionStorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..BODY_BYTES]).as_bytes() != &bytes[BODY_BYTES..] {
        return Err(SessionStorageError::CheckpointChecksum);
    }
    Ok(SessionCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| SessionStorageError::CheckpointLength)?,
        ),
        state_digest: bytes[24..56]
            .try_into()
            .map_err(|_| SessionStorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum SessionStorageError {
    #[error("shadow session error: {0}")]
    Session(#[from] SessionError),
    #[error("shadow session envelope error: {0:?}")]
    Schema(#[from] SchemaError),
    #[error("shadow session journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("shadow session segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("shadow session checkpoint I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("shadow session recovery mismatch")]
    RecoveryMismatch,
    #[error("shadow session sequence expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("shadow session sequence exhausted")]
    SequenceExhausted,
    #[error("shadow session envelope identity invalid")]
    EnvelopeIdentity,
    #[error("shadow session envelope timestamp invalid")]
    EnvelopeTimestamp,
    #[error("shadow session replay failed without halt")]
    Replay,
    #[error("shadow session post-halt event exists")]
    PostHaltEvent,
    #[error("shadow session checkpoint length invalid")]
    CheckpointLength,
    #[error("unsupported shadow session checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("shadow session checkpoint reserved bytes non-zero")]
    CheckpointReserved,
    #[error("shadow session checkpoint checksum invalid")]
    CheckpointChecksum,
    #[error("shadow session checkpoint mismatch")]
    CheckpointMismatch,
    #[error("shadow session checkpoint sequence missing")]
    CheckpointSequenceMissing,
    #[error("shadow session durable owner halted: {0}")]
    Halted(String),
}
