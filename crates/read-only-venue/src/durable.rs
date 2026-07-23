use crate::{
    decode_command, encode_command, Error as CoreError, ReadOnlyVenueSupervisor, VenueCommand,
    VenueOutcome, VenuePolicy,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::{fs, fs::OpenOptions, io::Write, path::Path};
use thiserror::Error;

const STREAM: &str = "__read_only_venue__";
const MAGIC: &[u8; 8] = b"POLYROC1";
const VERSION: u16 = 1;
const BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct VenueCheckpoint {
    pub sequence: u64,
    pub state_digest: [u8; 32],
}
#[derive(Debug)]
pub struct VenueRecovery {
    pub owner: ReadOnlyVenueSupervisor,
    pub last_sequence: Option<u64>,
}
#[derive(Debug)]
pub struct DurableReadOnlyVenue<J> {
    journal: J,
    owner: ReadOnlyVenueSupervisor,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableReadOnlyVenue<J> {
    /// Aligns a writer with recovered state.
    ///
    /// # Errors
    ///
    /// Rejects sequence disagreement.
    pub fn new(journal: J, recovery: VenueRecovery) -> Result<Self, VenueStorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(VenueStorageError::RecoveryMismatch);
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
    /// Returns state, encoding, journal, sequence, or poison failures.
    pub fn apply(&mut self, command: &VenueCommand) -> Result<VenueOutcome, VenueStorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(VenueStorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut next = self.owner.clone();
        let result = next.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |prior| {
            prior
                .checked_add(1)
                .ok_or(VenueStorageError::SequenceExhausted)
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
            return self.poison(VenueStorageError::Journal(error));
        }
        self.last_sequence = Some(sequence);
        if let Err(error) = self.journal.sync_events() {
            return self.poison(VenueStorageError::Journal(error));
        }
        self.owner = next;
        result.map_err(VenueStorageError::Core)
    }
    #[must_use]
    pub const fn owner(&self) -> &ReadOnlyVenueSupervisor {
        &self.owner
    }
    fn poison<T>(&mut self, error: VenueStorageError) -> Result<T, VenueStorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Strictly replays one segmented read-only venue journal.
///
/// # Errors
///
/// Rejects corruption, gaps, invalid envelopes, post-halt events and checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    policy: VenuePolicy,
    checkpoint: Option<VenueCheckpoint>,
) -> Result<VenueRecovery, VenueStorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut owner = ReadOnlyVenueSupervisor::new(policy)?;
    let mut expected = 0;
    let mut last = None;
    let mut verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(envelope) = reader.next_event()? {
        if halted {
            return Err(VenueStorageError::PostHaltEvent);
        }
        if envelope.sequence != expected {
            return Err(VenueStorageError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(VenueStorageError::SequenceExhausted)?;
        if envelope.source != EventSource::System || envelope.market_id != STREAM {
            return Err(VenueStorageError::EnvelopeIdentity);
        }
        let command = decode_command(&envelope.payload)?;
        if envelope.event_time_ns != command.recorded_at_ns()
            || envelope.received_time_ns != command.recorded_at_ns()
        {
            return Err(VenueStorageError::EnvelopeTimestamp);
        }
        if owner.apply(&command).is_err() {
            if !owner.is_halted() {
                return Err(VenueStorageError::Replay);
            }
            halted = true;
        }
        last = Some(envelope.sequence);
        if checkpoint.is_some_and(|value| value.sequence == envelope.sequence) {
            if owner.snapshot(command.recorded_at_ns()).digest
                != checkpoint
                    .ok_or(VenueStorageError::CheckpointMismatch)?
                    .state_digest
            {
                return Err(VenueStorageError::CheckpointMismatch);
            }
            verified = true;
        }
    }
    if !verified {
        return Err(VenueStorageError::CheckpointSequenceMissing);
    }
    Ok(VenueRecovery {
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
    checkpoint: VenueCheckpoint,
) -> Result<(), VenueStorageError> {
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
pub fn read_checkpoint(path: impl AsRef<Path>) -> Result<VenueCheckpoint, VenueStorageError> {
    let bytes = fs::read(path)?;
    if bytes.len() != CHECKPOINT_BYTES || bytes.get(..8) != Some(MAGIC) {
        return Err(VenueStorageError::CheckpointLength);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| VenueStorageError::CheckpointLength)?,
    );
    if version != VERSION {
        return Err(VenueStorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(VenueStorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..BODY_BYTES]).as_bytes() != &bytes[BODY_BYTES..] {
        return Err(VenueStorageError::CheckpointChecksum);
    }
    Ok(VenueCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| VenueStorageError::CheckpointLength)?,
        ),
        state_digest: bytes[24..56]
            .try_into()
            .map_err(|_| VenueStorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum VenueStorageError {
    #[error("venue core error: {0}")]
    Core(#[from] CoreError),
    #[error("venue schema error: {0}")]
    Schema(#[from] SchemaError),
    #[error("venue journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("venue segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("venue I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("venue recovery/writer mismatch")]
    RecoveryMismatch,
    #[error("venue sequence exhausted")]
    SequenceExhausted,
    #[error("venue sequence mismatch: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("venue envelope identity invalid")]
    EnvelopeIdentity,
    #[error("venue envelope timestamp invalid")]
    EnvelopeTimestamp,
    #[error("venue replay rejected transition")]
    Replay,
    #[error("venue event follows halt")]
    PostHaltEvent,
    #[error("venue checkpoint length or magic invalid")]
    CheckpointLength,
    #[error("unsupported venue checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("venue checkpoint reserved bytes non-zero")]
    CheckpointReserved,
    #[error("venue checkpoint checksum invalid")]
    CheckpointChecksum,
    #[error("venue checkpoint state digest mismatch")]
    CheckpointMismatch,
    #[error("venue checkpoint sequence absent from journal")]
    CheckpointSequenceMissing,
    #[error("durable venue owner halted: {0}")]
    Halted(String),
}
