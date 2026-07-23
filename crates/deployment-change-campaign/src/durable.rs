use crate::{
    decode_command, encode_command, CampaignCommand, CampaignOutcome, ChangeCampaignPolicy,
    DeploymentChangeCampaign, Error as CampaignError,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

const STREAM_ID: &str = "__deployment_change_campaign__";
const CHECKPOINT_MAGIC: &[u8; 8] = b"POLYDCP1";
const CHECKPOINT_VERSION: u16 = 1;
const CHECKPOINT_BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CampaignCheckpoint {
    pub sequence: u64,
    pub campaign_digest: [u8; 32],
}

#[derive(Debug)]
pub struct CampaignRecovery {
    pub campaign: DeploymentChangeCampaign,
    pub last_sequence: Option<u64>,
}

#[derive(Debug)]
pub struct DurableChangeCampaign<J> {
    journal: J,
    campaign: DeploymentChangeCampaign,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableChangeCampaign<J> {
    /// Aligns a journal writer with recovered campaign state.
    ///
    /// # Errors
    ///
    /// Rejects writer/recovery sequence disagreement.
    pub fn new(journal: J, recovery: CampaignRecovery) -> Result<Self, CampaignStorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(CampaignStorageError::RecoveryMismatch);
        }
        Ok(Self {
            journal,
            campaign: recovery.campaign,
            last_sequence: recovery.last_sequence,
            poisoned: None,
        })
    }

    /// Journals and device-syncs before installing one transition.
    ///
    /// # Errors
    ///
    /// Returns state, schema, journal, sequence or poisoned-owner errors.
    pub fn apply(
        &mut self,
        command: &CampaignCommand,
    ) -> Result<CampaignOutcome, CampaignStorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(CampaignStorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut owner = self.campaign.clone();
        let result = owner.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |value| {
            value
                .checked_add(1)
                .ok_or(CampaignStorageError::SequenceExhausted)
        })?;
        let timestamp = command.recorded_at_ns();
        let envelope = EventEnvelope::new(
            EventSource::System,
            sequence,
            timestamp,
            timestamp,
            STREAM_ID.to_owned(),
            payload,
        )?;
        if let Err(error) = self.journal.append_event(&envelope) {
            return self.poison(CampaignStorageError::Journal(error));
        }
        self.last_sequence = Some(sequence);
        if let Err(error) = self.journal.sync_events() {
            return self.poison(CampaignStorageError::Journal(error));
        }
        self.campaign = owner;
        result.map_err(CampaignStorageError::Campaign)
    }

    #[must_use]
    pub const fn campaign(&self) -> &DeploymentChangeCampaign {
        &self.campaign
    }

    fn poison<T>(&mut self, error: CampaignStorageError) -> Result<T, CampaignStorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Strictly replays one segmented campaign journal.
///
/// # Errors
///
/// Rejects corruption, gaps, invalid envelopes, post-halt events and checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    policy: ChangeCampaignPolicy,
    checkpoint: Option<CampaignCheckpoint>,
) -> Result<CampaignRecovery, CampaignStorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut owner = DeploymentChangeCampaign::new(policy)?;
    let mut expected = 0_u64;
    let mut last_sequence = None;
    let mut checkpoint_verified = checkpoint.is_none();
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
        let command = validate_envelope(&envelope)?;
        if let Err(error) = owner.apply(&command) {
            if !owner.is_halted() {
                return Err(CampaignStorageError::Campaign(error));
            }
            halted = true;
        }
        last_sequence = Some(envelope.sequence);
        if checkpoint.is_some_and(|value| value.sequence == envelope.sequence) {
            if owner.snapshot().digest
                != checkpoint
                    .map(|value| value.campaign_digest)
                    .ok_or(CampaignStorageError::CheckpointMismatch)?
            {
                return Err(CampaignStorageError::CheckpointMismatch);
            }
            checkpoint_verified = true;
        }
    }
    if !checkpoint_verified {
        return Err(CampaignStorageError::CheckpointSequenceMissing);
    }
    Ok(CampaignRecovery {
        campaign: owner,
        last_sequence,
    })
}

fn validate_envelope(envelope: &EventEnvelope) -> Result<CampaignCommand, CampaignStorageError> {
    if envelope.source != EventSource::System || envelope.market_id != STREAM_ID {
        return Err(CampaignStorageError::EnvelopeIdentity);
    }
    let command = decode_command(&envelope.payload)?;
    if envelope.event_time_ns != command.recorded_at_ns()
        || envelope.received_time_ns != command.recorded_at_ns()
    {
        return Err(CampaignStorageError::EnvelopeTimestamp);
    }
    Ok(command)
}

/// Creates and syncs one checkpoint without replacement.
///
/// # Errors
///
/// Returns I/O errors, including an existing target.
pub fn write_checkpoint_create_new(
    path: impl AsRef<Path>,
    checkpoint: CampaignCheckpoint,
) -> Result<(), CampaignStorageError> {
    let bytes = encode_checkpoint(checkpoint);
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
pub fn read_checkpoint(path: impl AsRef<Path>) -> Result<CampaignCheckpoint, CampaignStorageError> {
    decode_checkpoint(&fs::read(path)?)
}

fn encode_checkpoint(checkpoint: CampaignCheckpoint) -> [u8; CHECKPOINT_BYTES] {
    let mut bytes = [0_u8; CHECKPOINT_BYTES];
    bytes[0..8].copy_from_slice(CHECKPOINT_MAGIC);
    bytes[8..10].copy_from_slice(&CHECKPOINT_VERSION.to_le_bytes());
    bytes[16..24].copy_from_slice(&checkpoint.sequence.to_le_bytes());
    bytes[24..56].copy_from_slice(&checkpoint.campaign_digest);
    let checksum = blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]);
    bytes[CHECKPOINT_BODY_BYTES..].copy_from_slice(checksum.as_bytes());
    bytes
}

fn decode_checkpoint(bytes: &[u8]) -> Result<CampaignCheckpoint, CampaignStorageError> {
    if bytes.len() != CHECKPOINT_BYTES {
        return Err(CampaignStorageError::CheckpointLength);
    }
    if bytes.get(0..8) != Some(CHECKPOINT_MAGIC) {
        return Err(CampaignStorageError::CheckpointMagic);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| CampaignStorageError::CheckpointLength)?,
    );
    if version != CHECKPOINT_VERSION {
        return Err(CampaignStorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(CampaignStorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]).as_bytes() != &bytes[CHECKPOINT_BODY_BYTES..] {
        return Err(CampaignStorageError::CheckpointChecksum);
    }
    Ok(CampaignCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| CampaignStorageError::CheckpointLength)?,
        ),
        campaign_digest: bytes[24..56]
            .try_into()
            .map_err(|_| CampaignStorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum CampaignStorageError {
    #[error("change campaign error: {0}")]
    Campaign(#[from] CampaignError),
    #[error("change campaign envelope error: {0:?}")]
    Schema(#[from] SchemaError),
    #[error("change campaign journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("change campaign segmented journal error: {0}")]
    Segment(#[from] SegmentError),
    #[error("change campaign checkpoint I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("change campaign recovery sequence mismatch")]
    RecoveryMismatch,
    #[error("change campaign sequence exhausted")]
    SequenceExhausted,
    #[error("change campaign journal sequence mismatch: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("change campaign journal envelope identity is invalid")]
    EnvelopeIdentity,
    #[error("change campaign journal envelope timestamp is invalid")]
    EnvelopeTimestamp,
    #[error("change campaign checkpoint length is invalid")]
    CheckpointLength,
    #[error("change campaign checkpoint magic is invalid")]
    CheckpointMagic,
    #[error("unsupported change campaign checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("change campaign checkpoint reserved bytes are non-zero")]
    CheckpointReserved,
    #[error("change campaign checkpoint checksum is invalid")]
    CheckpointChecksum,
    #[error("change campaign checkpoint digest mismatch")]
    CheckpointMismatch,
    #[error("change campaign checkpoint sequence is missing")]
    CheckpointSequenceMissing,
    #[error("change campaign journal contains an event after halt")]
    PostHaltEvent,
    #[error("durable change campaign is halted: {0}")]
    Halted(String),
}
