use crate::{
    decode_command, encode_command, AuthCommand, AuthNoSubmitCertification, AuthOutcome,
    AuthPolicy, Error as CoreError,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::{fs, fs::OpenOptions, io::Write, path::Path};
use thiserror::Error;
const STREAM: &str = "__auth_no_submit__";
const MAGIC: &[u8; 8] = b"POLYANC1";
const VERSION: u16 = 1;
const BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct AuthCheckpoint {
    pub sequence: u64,
    pub state_digest: [u8; 32],
}
#[derive(Debug)]
pub struct AuthRecovery {
    pub owner: AuthNoSubmitCertification,
    pub last_sequence: Option<u64>,
}
#[derive(Debug)]
pub struct DurableAuthCertification<J> {
    journal: J,
    owner: AuthNoSubmitCertification,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}
impl<J: EventJournal> DurableAuthCertification<J> {
    /// Aligns a writer with recovered state.
    /// # Errors
    /// Rejects sequence disagreement.
    pub fn new(journal: J, recovery: AuthRecovery) -> Result<Self, AuthStorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(AuthStorageError::RecoveryMismatch);
        }
        Ok(Self {
            journal,
            owner: recovery.owner,
            last_sequence: recovery.last_sequence,
            poisoned: None,
        })
    }
    /// Journals and syncs before installing a transition.
    /// # Errors
    /// Returns encoding, journal, sequence, poison or core failures.
    pub fn apply(&mut self, command: &AuthCommand) -> Result<AuthOutcome, AuthStorageError> {
        if let Some(r) = &self.poisoned {
            return Err(AuthStorageError::Halted(r.clone()));
        }
        let payload = encode_command(command)?;
        let mut next = self.owner.clone();
        let result = next.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |v| {
            v.checked_add(1).ok_or(AuthStorageError::SequenceExhausted)
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
            return self.poison(AuthStorageError::Journal(e));
        }
        self.last_sequence = Some(sequence);
        if let Err(e) = self.journal.sync_events() {
            return self.poison(AuthStorageError::Journal(e));
        }
        self.owner = next;
        result.map_err(AuthStorageError::Core)
    }
    #[must_use]
    pub const fn owner(&self) -> &AuthNoSubmitCertification {
        &self.owner
    }
    fn poison<T>(&mut self, e: AuthStorageError) -> Result<T, AuthStorageError> {
        self.poisoned = Some(e.to_string());
        Err(e)
    }
}
/// Strictly replays a segmented journal.
/// # Errors
/// Rejects corruption, gaps, invalid envelopes, post-halt events and checkpoint mismatch.
pub fn recover_segmented(
    dir: impl AsRef<Path>,
    policy: AuthPolicy,
    checkpoint: Option<AuthCheckpoint>,
) -> Result<AuthRecovery, AuthStorageError> {
    let mut reader = SegmentedJournalReader::open(dir)?;
    let mut owner = AuthNoSubmitCertification::new(policy)?;
    let mut expected = 0;
    let mut last = None;
    let mut verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(e) = reader.next_event()? {
        if halted {
            return Err(AuthStorageError::PostHaltEvent);
        }
        if e.sequence != expected {
            return Err(AuthStorageError::Sequence {
                expected,
                actual: e.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(AuthStorageError::SequenceExhausted)?;
        if e.source != EventSource::System || e.market_id != STREAM {
            return Err(AuthStorageError::EnvelopeIdentity);
        }
        let c = decode_command(&e.payload)?;
        if e.event_time_ns != c.recorded_at_ns() || e.received_time_ns != c.recorded_at_ns() {
            return Err(AuthStorageError::EnvelopeTimestamp);
        }
        if owner.apply(&c).is_err() {
            if !owner.is_halted() {
                return Err(AuthStorageError::Replay);
            }
            halted = true;
        }
        last = Some(e.sequence);
        if checkpoint.is_some_and(|v| v.sequence == e.sequence) {
            if owner.snapshot().digest
                != checkpoint
                    .ok_or(AuthStorageError::CheckpointMismatch)?
                    .state_digest
            {
                return Err(AuthStorageError::CheckpointMismatch);
            }
            verified = true;
        }
    }
    if !verified {
        return Err(AuthStorageError::CheckpointSequenceMissing);
    }
    Ok(AuthRecovery {
        owner,
        last_sequence: last,
    })
}
/// Creates one synced checkpoint without replacement.
/// # Errors
/// Returns I/O errors, including existing output.
pub fn write_checkpoint_create_new(
    path: impl AsRef<Path>,
    c: AuthCheckpoint,
) -> Result<(), AuthStorageError> {
    let mut b = [0_u8; CHECKPOINT_BYTES];
    b[..8].copy_from_slice(MAGIC);
    b[8..10].copy_from_slice(&VERSION.to_le_bytes());
    b[16..24].copy_from_slice(&c.sequence.to_le_bytes());
    b[24..56].copy_from_slice(&c.state_digest);
    let sum = blake3::hash(&b[..BODY_BYTES]);
    b[BODY_BYTES..].copy_from_slice(sum.as_bytes());
    let mut f = OpenOptions::new().create_new(true).write(true).open(path)?;
    f.write_all(&b)?;
    f.sync_all()?;
    Ok(())
}
/// Reads one verified checkpoint.
/// # Errors
/// Rejects malformed, corrupt, unsupported or unreadable data.
pub fn read_checkpoint(path: impl AsRef<Path>) -> Result<AuthCheckpoint, AuthStorageError> {
    let b = fs::read(path)?;
    if b.len() != CHECKPOINT_BYTES || b.get(..8) != Some(MAGIC) {
        return Err(AuthStorageError::CheckpointLength);
    }
    let v = u16::from_le_bytes(
        b[8..10]
            .try_into()
            .map_err(|_| AuthStorageError::CheckpointLength)?,
    );
    if v != VERSION {
        return Err(AuthStorageError::CheckpointVersion(v));
    }
    if b[10..16] != [0; 6] {
        return Err(AuthStorageError::CheckpointReserved);
    }
    if blake3::hash(&b[..BODY_BYTES]).as_bytes() != &b[BODY_BYTES..] {
        return Err(AuthStorageError::CheckpointChecksum);
    }
    Ok(AuthCheckpoint {
        sequence: u64::from_le_bytes(
            b[16..24]
                .try_into()
                .map_err(|_| AuthStorageError::CheckpointLength)?,
        ),
        state_digest: b[24..56]
            .try_into()
            .map_err(|_| AuthStorageError::CheckpointLength)?,
    })
}
#[derive(Debug, Error)]
pub enum AuthStorageError {
    #[error("auth core error: {0}")]
    Core(#[from] CoreError),
    #[error("auth schema error: {0}")]
    Schema(#[from] SchemaError),
    #[error("auth journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("auth segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("auth I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("auth recovery mismatch")]
    RecoveryMismatch,
    #[error("auth sequence exhausted")]
    SequenceExhausted,
    #[error("auth sequence gap: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("auth envelope identity invalid")]
    EnvelopeIdentity,
    #[error("auth envelope timestamp invalid")]
    EnvelopeTimestamp,
    #[error("auth replay failure")]
    Replay,
    #[error("auth post-halt event")]
    PostHaltEvent,
    #[error("auth checkpoint length invalid")]
    CheckpointLength,
    #[error("auth checkpoint version {0}")]
    CheckpointVersion(u16),
    #[error("auth checkpoint reserved bytes")]
    CheckpointReserved,
    #[error("auth checkpoint checksum")]
    CheckpointChecksum,
    #[error("auth checkpoint mismatch")]
    CheckpointMismatch,
    #[error("auth checkpoint sequence missing")]
    CheckpointSequenceMissing,
    #[error("auth durable halted: {0}")]
    Halted(String),
}
