use crate::{
    decode_command, encode_command, CampaignCommand, CampaignOutcome, CampaignPolicy,
    Error as CampaignError, ShadowSessionCampaign,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use settlement_reconciliation::ReconcilerConfig;
use shadow_gateway_harness::GatewayConfig;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

const STREAM_ID: &str = "__shadow_session_campaign__";
const CHECKPOINT_MAGIC: &[u8; 8] = b"POLYSSC1";
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
    pub campaign: ShadowSessionCampaign,
    pub last_sequence: Option<u64>,
}

#[derive(Debug)]
pub struct DurableCampaignRunner<J> {
    journal: J,
    campaign: ShadowSessionCampaign,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableCampaignRunner<J> {
    /// Aligns a journal writer with recovered campaign state.
    ///
    /// # Errors
    ///
    /// Rejects writer/recovery sequence disagreement.
    pub fn new(journal: J, recovery: CampaignRecovery) -> Result<Self, StorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(StorageError::RecoveryMismatch);
        }
        Ok(Self {
            journal,
            campaign: recovery.campaign,
            last_sequence: recovery.last_sequence,
            poisoned: None,
        })
    }

    /// Appends and device-syncs before installing one campaign transition.
    ///
    /// # Errors
    ///
    /// Returns campaign, schema, journal, sequence, or poisoned-owner failures.
    pub fn apply(&mut self, command: &CampaignCommand) -> Result<CampaignOutcome, StorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(StorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut preflight = self.campaign.clone();
        let result = preflight.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |value| {
            value.checked_add(1).ok_or(StorageError::SequenceExhausted)
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
            return self.poison(StorageError::Journal(error));
        }
        self.last_sequence = Some(sequence);
        if let Err(error) = self.journal.sync_events() {
            return self.poison(StorageError::Journal(error));
        }
        self.campaign = preflight;
        result.map_err(StorageError::Campaign)
    }

    #[must_use]
    pub const fn campaign(&self) -> &ShadowSessionCampaign {
        &self.campaign
    }

    fn poison<T>(&mut self, error: StorageError) -> Result<T, StorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Strictly replays a segmented campaign journal.
///
/// # Errors
///
/// Rejects corruption, gaps, invalid envelopes, post-halt events, and
/// checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    policy: CampaignPolicy,
    gateway: GatewayConfig,
    reconciliation: ReconcilerConfig,
    checkpoint: Option<CampaignCheckpoint>,
) -> Result<CampaignRecovery, StorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut campaign = ShadowSessionCampaign::new(policy, gateway, reconciliation)?;
    let mut expected = 0_u64;
    let mut last_sequence = None;
    let mut checkpoint_verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(envelope) = reader.next_event()? {
        if halted {
            return Err(StorageError::PostHaltEvent);
        }
        if envelope.sequence != expected {
            return Err(StorageError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(StorageError::SequenceExhausted)?;
        let command = validate_envelope(&envelope)?;
        if let Err(error) = campaign.apply(&command) {
            if !campaign.is_halted() {
                return Err(StorageError::Campaign(error));
            }
            halted = true;
        }
        last_sequence = Some(envelope.sequence);
        if checkpoint.is_some_and(|value| value.sequence == envelope.sequence) {
            if campaign.snapshot().digest
                != checkpoint
                    .map(|value| value.campaign_digest)
                    .ok_or(StorageError::CheckpointMismatch)?
            {
                return Err(StorageError::CheckpointMismatch);
            }
            checkpoint_verified = true;
        }
    }
    if !checkpoint_verified {
        return Err(StorageError::CheckpointSequenceMissing);
    }
    Ok(CampaignRecovery {
        campaign,
        last_sequence,
    })
}

fn validate_envelope(envelope: &EventEnvelope) -> Result<CampaignCommand, StorageError> {
    if envelope.source != EventSource::System || envelope.market_id != STREAM_ID {
        return Err(StorageError::EnvelopeIdentity);
    }
    let command = decode_command(&envelope.payload)?;
    if envelope.event_time_ns != command.recorded_at_ns()
        || envelope.received_time_ns != command.recorded_at_ns()
    {
        return Err(StorageError::EnvelopeTimestamp);
    }
    Ok(command)
}

/// Creates and syncs a new campaign checkpoint without replacement.
///
/// # Errors
///
/// Returns I/O errors, including an existing target.
pub fn write_checkpoint_create_new(
    path: impl AsRef<Path>,
    checkpoint: CampaignCheckpoint,
) -> Result<(), StorageError> {
    let bytes = encode_checkpoint(checkpoint);
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Reads and verifies one campaign checkpoint.
///
/// # Errors
///
/// Rejects length, magic, version, reserved bytes, checksum, and I/O failures.
pub fn read_checkpoint(path: impl AsRef<Path>) -> Result<CampaignCheckpoint, StorageError> {
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

fn decode_checkpoint(bytes: &[u8]) -> Result<CampaignCheckpoint, StorageError> {
    if bytes.len() != CHECKPOINT_BYTES {
        return Err(StorageError::CheckpointLength);
    }
    if bytes.get(0..8) != Some(CHECKPOINT_MAGIC) {
        return Err(StorageError::CheckpointMagic);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| StorageError::CheckpointLength)?,
    );
    if version != CHECKPOINT_VERSION {
        return Err(StorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(StorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]).as_bytes() != &bytes[CHECKPOINT_BODY_BYTES..] {
        return Err(StorageError::CheckpointChecksum);
    }
    Ok(CampaignCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| StorageError::CheckpointLength)?,
        ),
        campaign_digest: bytes[24..56]
            .try_into()
            .map_err(|_| StorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("campaign error: {0}")]
    Campaign(#[from] CampaignError),
    #[error("campaign envelope error: {0:?}")]
    Schema(#[from] SchemaError),
    #[error("campaign journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("campaign segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("campaign I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("campaign sequence mismatch: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("campaign sequence is exhausted")]
    SequenceExhausted,
    #[error("campaign envelope identity is invalid")]
    EnvelopeIdentity,
    #[error("campaign envelope timestamp is invalid")]
    EnvelopeTimestamp,
    #[error("campaign recovery does not match the journal writer")]
    RecoveryMismatch,
    #[error("campaign owner is halted: {0}")]
    Halted(String),
    #[error("campaign checkpoint length is invalid")]
    CheckpointLength,
    #[error("campaign checkpoint magic is invalid")]
    CheckpointMagic,
    #[error("unsupported campaign checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("campaign checkpoint reserved bytes are non-zero")]
    CheckpointReserved,
    #[error("campaign checkpoint checksum is invalid")]
    CheckpointChecksum,
    #[error("campaign checkpoint digest does not match replay")]
    CheckpointMismatch,
    #[error("campaign checkpoint sequence was not present")]
    CheckpointSequenceMissing,
    #[error("campaign journal contains an event after terminal halt")]
    PostHaltEvent,
}
