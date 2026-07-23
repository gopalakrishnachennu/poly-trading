use crate::{
    decode_command, encode_command, Error as CoreError, LiveDataPaperCertification, PaperCommand,
    PaperOutcomeRecord, PaperPolicy,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::{fs, fs::OpenOptions, io::Write, path::Path};
use thiserror::Error;
const STREAM: &str = "__live_data_paper_cert__";
const MAGIC: &[u8; 8] = b"POLYLPC1";
const VERSION: u16 = 1;
const BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PaperCheckpoint {
    pub sequence: u64,
    pub state_digest: [u8; 32],
}
#[derive(Debug)]
pub struct PaperRecovery {
    pub owner: LiveDataPaperCertification,
    pub last_sequence: Option<u64>,
}
#[derive(Debug)]
pub struct DurablePaperCertification<J> {
    journal: J,
    owner: LiveDataPaperCertification,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}
impl<J: EventJournal> DurablePaperCertification<J> {
    /// Aligns a writer with recovered state.
    /// # Errors
    /// Rejects sequence disagreement.
    pub fn new(journal: J, recovery: PaperRecovery) -> Result<Self, PaperStorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(PaperStorageError::RecoveryMismatch);
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
    /// Returns encoding, journal, sequence, poison or core failure.
    pub fn apply(
        &mut self,
        command: &PaperCommand,
    ) -> Result<PaperOutcomeRecord, PaperStorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(PaperStorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut next = self.owner.clone();
        let result = next.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |v| {
            v.checked_add(1).ok_or(PaperStorageError::SequenceExhausted)
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
            return self.poison(PaperStorageError::Journal(e));
        }
        self.last_sequence = Some(sequence);
        if let Err(e) = self.journal.sync_events() {
            return self.poison(PaperStorageError::Journal(e));
        }
        self.owner = next;
        result.map_err(PaperStorageError::Core)
    }
    #[must_use]
    pub const fn owner(&self) -> &LiveDataPaperCertification {
        &self.owner
    }
    fn poison<T>(&mut self, error: PaperStorageError) -> Result<T, PaperStorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}
/// Strictly replays a segmented certification journal.
/// # Errors
/// Rejects corruption, gaps, invalid envelopes, post-halt events and checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    policy: PaperPolicy,
    checkpoint: Option<PaperCheckpoint>,
) -> Result<PaperRecovery, PaperStorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut owner = LiveDataPaperCertification::new(policy)?;
    let mut expected = 0;
    let mut last = None;
    let mut verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(envelope) = reader.next_event()? {
        if halted {
            return Err(PaperStorageError::PostHaltEvent);
        }
        if envelope.sequence != expected {
            return Err(PaperStorageError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(PaperStorageError::SequenceExhausted)?;
        if envelope.source != EventSource::System || envelope.market_id != STREAM {
            return Err(PaperStorageError::EnvelopeIdentity);
        }
        let command = decode_command(&envelope.payload)?;
        if envelope.event_time_ns != command.recorded_at_ns()
            || envelope.received_time_ns != command.recorded_at_ns()
        {
            return Err(PaperStorageError::EnvelopeTimestamp);
        }
        if owner.apply(&command).is_err() {
            if !owner.is_halted() {
                return Err(PaperStorageError::Replay);
            }
            halted = true;
        }
        last = Some(envelope.sequence);
        if checkpoint.is_some_and(|v| v.sequence == envelope.sequence) {
            if owner.snapshot().digest
                != checkpoint
                    .ok_or(PaperStorageError::CheckpointMismatch)?
                    .state_digest
            {
                return Err(PaperStorageError::CheckpointMismatch);
            }
            verified = true;
        }
    }
    if !verified {
        return Err(PaperStorageError::CheckpointSequenceMissing);
    }
    Ok(PaperRecovery {
        owner,
        last_sequence: last,
    })
}
/// Creates one synced checkpoint without replacement.
/// # Errors
/// Returns I/O errors, including existing output.
pub fn write_checkpoint_create_new(
    path: impl AsRef<Path>,
    checkpoint: PaperCheckpoint,
) -> Result<(), PaperStorageError> {
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
pub fn read_checkpoint(path: impl AsRef<Path>) -> Result<PaperCheckpoint, PaperStorageError> {
    let bytes = fs::read(path)?;
    if bytes.len() != CHECKPOINT_BYTES || bytes.get(..8) != Some(MAGIC) {
        return Err(PaperStorageError::CheckpointLength);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| PaperStorageError::CheckpointLength)?,
    );
    if version != VERSION {
        return Err(PaperStorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(PaperStorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..BODY_BYTES]).as_bytes() != &bytes[BODY_BYTES..] {
        return Err(PaperStorageError::CheckpointChecksum);
    }
    Ok(PaperCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| PaperStorageError::CheckpointLength)?,
        ),
        state_digest: bytes[24..56]
            .try_into()
            .map_err(|_| PaperStorageError::CheckpointLength)?,
    })
}
#[derive(Debug, Error)]
pub enum PaperStorageError {
    #[error("paper core error: {0}")]
    Core(#[from] CoreError),
    #[error("paper schema error: {0}")]
    Schema(#[from] SchemaError),
    #[error("paper journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("paper segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("paper checkpoint I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("paper writer recovery mismatch")]
    RecoveryMismatch,
    #[error("paper journal sequence exhausted")]
    SequenceExhausted,
    #[error("paper sequence gap: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("paper envelope identity invalid")]
    EnvelopeIdentity,
    #[error("paper envelope timestamp invalid")]
    EnvelopeTimestamp,
    #[error("paper replay failed without halt")]
    Replay,
    #[error("paper event follows halt")]
    PostHaltEvent,
    #[error("paper checkpoint length or magic invalid")]
    CheckpointLength,
    #[error("unsupported paper checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("paper checkpoint reserved bytes non-zero")]
    CheckpointReserved,
    #[error("paper checkpoint checksum invalid")]
    CheckpointChecksum,
    #[error("paper checkpoint mismatch")]
    CheckpointMismatch,
    #[error("paper checkpoint sequence missing")]
    CheckpointSequenceMissing,
    #[error("paper durable writer halted: {0}")]
    Halted(String),
}
