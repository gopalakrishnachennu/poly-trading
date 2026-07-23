use crate::{
    decode_command, encode_command, ChainCommand, ChainObserver, ChainOutcome, ChainPolicy,
    Error as CoreError,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::{fs, fs::OpenOptions, io::Write, path::Path};
use thiserror::Error;

const STREAM: &str = "__chain_observer__";
const MAGIC: &[u8; 8] = b"POLYCOC1";
const VERSION: u16 = 1;
const BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChainCheckpoint {
    pub sequence: u64,
    pub state_digest: [u8; 32],
}
#[derive(Debug)]
pub struct ChainRecovery {
    pub owner: ChainObserver,
    pub last_sequence: Option<u64>,
}
#[derive(Debug)]
pub struct DurableChainObserver<J> {
    journal: J,
    owner: ChainObserver,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableChainObserver<J> {
    /// Aligns a writer with recovered state.
    /// # Errors
    /// Rejects journal sequence disagreement.
    pub fn new(journal: J, recovery: ChainRecovery) -> Result<Self, ChainStorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(ChainStorageError::RecoveryMismatch);
        }
        Ok(Self {
            journal,
            owner: recovery.owner,
            last_sequence: recovery.last_sequence,
            poisoned: None,
        })
    }
    /// Journals and syncs before installing one transition.
    /// # Errors
    /// Returns encoding, journal, sequence, poison or core failures.
    pub fn apply(&mut self, command: &ChainCommand) -> Result<ChainOutcome, ChainStorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(ChainStorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut next = self.owner.clone();
        let result = next.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |v| {
            v.checked_add(1).ok_or(ChainStorageError::SequenceExhausted)
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
            return self.poison(ChainStorageError::Journal(error));
        }
        self.last_sequence = Some(sequence);
        if let Err(error) = self.journal.sync_events() {
            return self.poison(ChainStorageError::Journal(error));
        }
        self.owner = next;
        result.map_err(ChainStorageError::Core)
    }
    #[must_use]
    pub const fn owner(&self) -> &ChainObserver {
        &self.owner
    }
    fn poison<T>(&mut self, error: ChainStorageError) -> Result<T, ChainStorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Strictly replays one segmented chain-observer journal.
/// # Errors
/// Rejects corruption, gaps, invalid envelopes, post-halt events and checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    policy: ChainPolicy,
    checkpoint: Option<ChainCheckpoint>,
) -> Result<ChainRecovery, ChainStorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut owner = ChainObserver::new(policy)?;
    let mut expected = 0;
    let mut last = None;
    let mut verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(envelope) = reader.next_event()? {
        if halted {
            return Err(ChainStorageError::PostHaltEvent);
        }
        if envelope.sequence != expected {
            return Err(ChainStorageError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(ChainStorageError::SequenceExhausted)?;
        if envelope.source != EventSource::System || envelope.market_id != STREAM {
            return Err(ChainStorageError::EnvelopeIdentity);
        }
        let command = decode_command(&envelope.payload)?;
        if envelope.event_time_ns != command.recorded_at_ns()
            || envelope.received_time_ns != command.recorded_at_ns()
        {
            return Err(ChainStorageError::EnvelopeTimestamp);
        }
        if owner.apply(&command).is_err() {
            if !owner.is_halted() {
                return Err(ChainStorageError::Replay);
            }
            halted = true;
        }
        last = Some(envelope.sequence);
        if checkpoint.is_some_and(|v| v.sequence == envelope.sequence) {
            if owner.snapshot(command.recorded_at_ns()).digest
                != checkpoint
                    .ok_or(ChainStorageError::CheckpointMismatch)?
                    .state_digest
            {
                return Err(ChainStorageError::CheckpointMismatch);
            }
            verified = true;
        }
    }
    if !verified {
        return Err(ChainStorageError::CheckpointSequenceMissing);
    }
    Ok(ChainRecovery {
        owner,
        last_sequence: last,
    })
}

/// Creates and syncs one checkpoint without replacement.
/// # Errors
/// Returns I/O errors, including an existing target.
pub fn write_checkpoint_create_new(
    path: impl AsRef<Path>,
    checkpoint: ChainCheckpoint,
) -> Result<(), ChainStorageError> {
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
/// # Errors
/// Rejects length, magic, version, reserved bytes, checksum and I/O failures.
pub fn read_checkpoint(path: impl AsRef<Path>) -> Result<ChainCheckpoint, ChainStorageError> {
    let bytes = fs::read(path)?;
    if bytes.len() != CHECKPOINT_BYTES || bytes.get(..8) != Some(MAGIC) {
        return Err(ChainStorageError::CheckpointLength);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| ChainStorageError::CheckpointLength)?,
    );
    if version != VERSION {
        return Err(ChainStorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(ChainStorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..BODY_BYTES]).as_bytes() != &bytes[BODY_BYTES..] {
        return Err(ChainStorageError::CheckpointChecksum);
    }
    Ok(ChainCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| ChainStorageError::CheckpointLength)?,
        ),
        state_digest: bytes[24..56]
            .try_into()
            .map_err(|_| ChainStorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum ChainStorageError {
    #[error("chain core error: {0}")]
    Core(#[from] CoreError),
    #[error("chain schema error: {0}")]
    Schema(#[from] SchemaError),
    #[error("chain journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("chain segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("chain checkpoint I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("chain writer does not match recovered sequence")]
    RecoveryMismatch,
    #[error("chain journal sequence exhausted")]
    SequenceExhausted,
    #[error("chain journal sequence gap: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("chain journal envelope identity invalid")]
    EnvelopeIdentity,
    #[error("chain journal envelope timestamp invalid")]
    EnvelopeTimestamp,
    #[error("chain replay failed without absorbing halt")]
    Replay,
    #[error("chain event follows absorbing halt")]
    PostHaltEvent,
    #[error("chain checkpoint length or magic invalid")]
    CheckpointLength,
    #[error("unsupported chain checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("chain checkpoint reserved bytes non-zero")]
    CheckpointReserved,
    #[error("chain checkpoint checksum invalid")]
    CheckpointChecksum,
    #[error("chain checkpoint does not match replayed state")]
    CheckpointMismatch,
    #[error("chain checkpoint sequence missing from journal")]
    CheckpointSequenceMissing,
    #[error("chain durable writer halted: {0}")]
    Halted(String),
}
