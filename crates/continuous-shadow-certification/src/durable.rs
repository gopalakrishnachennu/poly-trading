use crate::{
    decode_command, encode_command, CampaignCommand, CampaignOutcome, CampaignPolicy,
    ContinuousCampaign, Error as CoreError,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::{fs, fs::OpenOptions, io::Write, path::Path};
use thiserror::Error;

const STREAM: &str = "__continuous_shadow_campaign__";
const MAGIC: &[u8; 8] = b"POLYCSC1";
const VERSION: u16 = 1;
const BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CampaignCheckpoint {
    pub sequence: u64,
    pub state_digest: [u8; 32],
}
#[derive(Debug)]
pub struct CampaignRecovery {
    pub owner: ContinuousCampaign,
    pub last_sequence: Option<u64>,
}
#[derive(Debug)]
pub struct DurableCampaign<J> {
    journal: J,
    owner: ContinuousCampaign,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}
impl<J: EventJournal> DurableCampaign<J> {
    /// Aligns the writer with recovered campaign state.
    /// # Errors
    /// Rejects journal sequence disagreement.
    pub fn new(journal: J, recovery: CampaignRecovery) -> Result<Self, CampaignStorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(CampaignStorageError::RecoveryMismatch);
        }
        Ok(Self {
            journal,
            owner: recovery.owner,
            last_sequence: recovery.last_sequence,
            poisoned: None,
        })
    }
    /// Journals and syncs before installing one campaign transition.
    /// # Errors
    /// Returns encoding, journal, sequence, poison or core failures.
    pub fn apply(
        &mut self,
        command: &CampaignCommand,
    ) -> Result<CampaignOutcome, CampaignStorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(CampaignStorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut next = self.owner.clone();
        let result = next.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |v| {
            v.checked_add(1)
                .ok_or(CampaignStorageError::SequenceExhausted)
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
            return self.poison(CampaignStorageError::Journal(error));
        }
        self.last_sequence = Some(sequence);
        if let Err(error) = self.journal.sync_events() {
            return self.poison(CampaignStorageError::Journal(error));
        }
        self.owner = next;
        result.map_err(CampaignStorageError::Core)
    }
    #[must_use]
    pub const fn owner(&self) -> &ContinuousCampaign {
        &self.owner
    }
    fn poison<T>(&mut self, error: CampaignStorageError) -> Result<T, CampaignStorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Strictly replays one segmented campaign journal.
/// # Errors
/// Rejects corruption, gaps, invalid envelopes, post-halt events and checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    policy: CampaignPolicy,
    checkpoint: Option<CampaignCheckpoint>,
) -> Result<CampaignRecovery, CampaignStorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut owner = ContinuousCampaign::new(policy)?;
    let mut expected = 0;
    let mut last = None;
    let mut verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(envelope) = reader.next_event()? {
        if halted {
            return Err(CampaignStorageError::PostHaltEvent);
        }
        if envelope.sequence != expected {
            return Err(CampaignStorageError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(CampaignStorageError::SequenceExhausted)?;
        if envelope.source != EventSource::System || envelope.market_id != STREAM {
            return Err(CampaignStorageError::EnvelopeIdentity);
        }
        let command = decode_command(&envelope.payload)?;
        if envelope.event_time_ns != command.recorded_at_ns()
            || envelope.received_time_ns != command.recorded_at_ns()
        {
            return Err(CampaignStorageError::EnvelopeTimestamp);
        }
        if owner.apply(&command).is_err() {
            if !owner.is_halted() {
                return Err(CampaignStorageError::Replay);
            }
            halted = true;
        }
        last = Some(envelope.sequence);
        if checkpoint.is_some_and(|v| v.sequence == envelope.sequence) {
            if owner.snapshot(command.recorded_at_ns()).digest
                != checkpoint
                    .ok_or(CampaignStorageError::CheckpointMismatch)?
                    .state_digest
            {
                return Err(CampaignStorageError::CheckpointMismatch);
            }
            verified = true;
        }
    }
    if !verified {
        return Err(CampaignStorageError::CheckpointSequenceMissing);
    }
    Ok(CampaignRecovery {
        owner,
        last_sequence: last,
    })
}

/// Creates and syncs one checkpoint without replacement.
/// # Errors
/// Returns I/O errors, including an existing target.
pub fn write_checkpoint_create_new(
    path: impl AsRef<Path>,
    checkpoint: CampaignCheckpoint,
) -> Result<(), CampaignStorageError> {
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
pub fn read_checkpoint(path: impl AsRef<Path>) -> Result<CampaignCheckpoint, CampaignStorageError> {
    let bytes = fs::read(path)?;
    if bytes.len() != CHECKPOINT_BYTES || bytes.get(..8) != Some(MAGIC) {
        return Err(CampaignStorageError::CheckpointLength);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| CampaignStorageError::CheckpointLength)?,
    );
    if version != VERSION {
        return Err(CampaignStorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(CampaignStorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..BODY_BYTES]).as_bytes() != &bytes[BODY_BYTES..] {
        return Err(CampaignStorageError::CheckpointChecksum);
    }
    Ok(CampaignCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| CampaignStorageError::CheckpointLength)?,
        ),
        state_digest: bytes[24..56]
            .try_into()
            .map_err(|_| CampaignStorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum CampaignStorageError {
    #[error("campaign core error: {0}")]
    Core(#[from] CoreError),
    #[error("campaign schema error: {0}")]
    Schema(#[from] SchemaError),
    #[error("campaign journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("campaign segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("campaign checkpoint I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("campaign writer does not match recovered sequence")]
    RecoveryMismatch,
    #[error("campaign journal sequence exhausted")]
    SequenceExhausted,
    #[error("campaign journal sequence gap: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("campaign journal envelope identity invalid")]
    EnvelopeIdentity,
    #[error("campaign journal envelope timestamp invalid")]
    EnvelopeTimestamp,
    #[error("campaign replay failed without absorbing halt")]
    Replay,
    #[error("campaign event follows absorbing halt")]
    PostHaltEvent,
    #[error("campaign checkpoint length or magic invalid")]
    CheckpointLength,
    #[error("unsupported campaign checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("campaign checkpoint reserved bytes non-zero")]
    CheckpointReserved,
    #[error("campaign checkpoint checksum invalid")]
    CheckpointChecksum,
    #[error("campaign checkpoint does not match replayed state")]
    CheckpointMismatch,
    #[error("campaign checkpoint sequence missing from journal")]
    CheckpointSequenceMissing,
    #[error("campaign durable writer halted: {0}")]
    Halted(String),
}
