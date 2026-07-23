use crate::{
    decode_command, encode_command, ControlledProductionRelease, Error as CoreError,
    ReleaseCommand, ReleaseOutcome, ReleasePolicy,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::{fs, fs::OpenOptions, io::Write, path::Path};
use thiserror::Error;

const STREAM: &str = "__controlled_production_release__";
const MAGIC: &[u8; 8] = b"POLYCPR1";
const VERSION: u16 = 1;
const BODY: usize = 56;
const SIZE: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReleaseCheckpoint {
    pub sequence: u64,
    pub state_digest: [u8; 32],
}

#[derive(Debug)]
pub struct ReleaseRecovery {
    pub owner: ControlledProductionRelease,
    pub last_sequence: Option<u64>,
}

#[derive(Debug)]
pub struct DurableReleaseController<J> {
    journal: J,
    owner: ControlledProductionRelease,
    last: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableReleaseController<J> {
    /// Aligns a journal with recovered state.
    /// # Errors
    /// Rejects sequence disagreement.
    pub fn new(journal: J, recovery: ReleaseRecovery) -> Result<Self, ReleaseStorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(ReleaseStorageError::RecoveryMismatch);
        }
        Ok(Self {
            journal,
            owner: recovery.owner,
            last: recovery.last_sequence,
            poisoned: None,
        })
    }

    /// Journals before changing authoritative state.
    /// # Errors
    /// Returns storage or deterministic core failures.
    pub fn apply(
        &mut self,
        command: &ReleaseCommand,
    ) -> Result<ReleaseOutcome, ReleaseStorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(ReleaseStorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut next = self.owner.clone();
        let result = next.apply(command);
        let sequence = self.last.map_or(Ok(0), |v| {
            v.checked_add(1)
                .ok_or(ReleaseStorageError::SequenceExhausted)
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
            return self.poison(ReleaseStorageError::Journal(error));
        }
        self.last = Some(sequence);
        if let Err(error) = self.journal.sync_events() {
            return self.poison(ReleaseStorageError::Journal(error));
        }
        self.owner = next;
        result.map_err(ReleaseStorageError::Core)
    }

    #[must_use]
    pub const fn owner(&self) -> &ControlledProductionRelease {
        &self.owner
    }

    fn poison<T>(&mut self, error: ReleaseStorageError) -> Result<T, ReleaseStorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Replays a segmented release journal.
/// # Errors
/// Rejects corruption, gaps, invalid envelopes, post-halt state and checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    policy: ReleasePolicy,
    checkpoint: Option<ReleaseCheckpoint>,
) -> Result<ReleaseRecovery, ReleaseStorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut owner = ControlledProductionRelease::new(policy)?;
    let mut expected = 0_u64;
    let mut last = None;
    let mut verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(envelope) = reader.next_event()? {
        if halted {
            return Err(ReleaseStorageError::PostHaltEvent);
        }
        if envelope.sequence != expected {
            return Err(ReleaseStorageError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(ReleaseStorageError::SequenceExhausted)?;
        if envelope.source != EventSource::System || envelope.market_id != STREAM {
            return Err(ReleaseStorageError::EnvelopeIdentity);
        }
        let command = decode_command(&envelope.payload)?;
        if envelope.event_time_ns != command.recorded_at_ns()
            || envelope.received_time_ns != command.recorded_at_ns()
        {
            return Err(ReleaseStorageError::EnvelopeTimestamp);
        }
        if owner.apply(&command).is_err() {
            if !owner.is_halted() {
                return Err(ReleaseStorageError::Replay);
            }
            halted = true;
        }
        last = Some(envelope.sequence);
        if checkpoint.is_some_and(|v| v.sequence == envelope.sequence) {
            if owner.snapshot().digest
                != checkpoint
                    .ok_or(ReleaseStorageError::CheckpointMismatch)?
                    .state_digest
            {
                return Err(ReleaseStorageError::CheckpointMismatch);
            }
            verified = true;
        }
    }
    if !verified {
        return Err(ReleaseStorageError::CheckpointSequenceMissing);
    }
    Ok(ReleaseRecovery {
        owner,
        last_sequence: last,
    })
}

/// Creates a checksummed checkpoint without overwriting.
/// # Errors
/// Returns I/O failures, including an existing target.
pub fn write_checkpoint_create_new(
    path: impl AsRef<Path>,
    checkpoint: ReleaseCheckpoint,
) -> Result<(), ReleaseStorageError> {
    let mut bytes = [0_u8; SIZE];
    bytes[..8].copy_from_slice(MAGIC);
    bytes[8..10].copy_from_slice(&VERSION.to_le_bytes());
    bytes[16..24].copy_from_slice(&checkpoint.sequence.to_le_bytes());
    bytes[24..56].copy_from_slice(&checkpoint.state_digest);
    let checksum = blake3::hash(&bytes[..BODY]);
    bytes[BODY..].copy_from_slice(checksum.as_bytes());
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Reads and verifies a release checkpoint.
/// # Errors
/// Rejects malformed or corrupt data.
pub fn read_checkpoint(path: impl AsRef<Path>) -> Result<ReleaseCheckpoint, ReleaseStorageError> {
    let bytes = fs::read(path)?;
    if bytes.len() != SIZE || bytes.get(..8) != Some(MAGIC) {
        return Err(ReleaseStorageError::CheckpointLength);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| ReleaseStorageError::CheckpointLength)?,
    );
    if version != VERSION {
        return Err(ReleaseStorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(ReleaseStorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..BODY]).as_bytes() != &bytes[BODY..] {
        return Err(ReleaseStorageError::CheckpointChecksum);
    }
    Ok(ReleaseCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| ReleaseStorageError::CheckpointLength)?,
        ),
        state_digest: bytes[24..56]
            .try_into()
            .map_err(|_| ReleaseStorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum ReleaseStorageError {
    #[error("release core: {0}")]
    Core(#[from] CoreError),
    #[error("release schema: {0}")]
    Schema(#[from] SchemaError),
    #[error("release journal: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("release segment: {0}")]
    Segment(#[from] SegmentError),
    #[error("release I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("recovery mismatch")]
    RecoveryMismatch,
    #[error("sequence exhausted")]
    SequenceExhausted,
    #[error("sequence expected {expected} actual {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("envelope identity")]
    EnvelopeIdentity,
    #[error("envelope timestamp")]
    EnvelopeTimestamp,
    #[error("replay failure")]
    Replay,
    #[error("post-halt event")]
    PostHaltEvent,
    #[error("checkpoint mismatch")]
    CheckpointMismatch,
    #[error("checkpoint sequence missing")]
    CheckpointSequenceMissing,
    #[error("checkpoint length")]
    CheckpointLength,
    #[error("checkpoint version {0}")]
    CheckpointVersion(u16),
    #[error("checkpoint reserved")]
    CheckpointReserved,
    #[error("checkpoint checksum")]
    CheckpointChecksum,
    #[error("durable release halted: {0}")]
    Halted(String),
}
