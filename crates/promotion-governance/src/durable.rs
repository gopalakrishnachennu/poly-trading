use crate::{
    decode_command, encode_command, Error as GovernanceError, GovernanceCommand, GovernanceOutcome,
    GovernancePolicy, PromotionGovernance,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

const STREAM_ID: &str = "__promotion_governance__";
const CHECKPOINT_MAGIC: &[u8; 8] = b"POLYPGC1";
const CHECKPOINT_VERSION: u16 = 1;
const CHECKPOINT_BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GovernanceCheckpoint {
    pub sequence: u64,
    pub governance_digest: [u8; 32],
}

#[derive(Debug)]
pub struct GovernanceRecovery {
    pub governance: PromotionGovernance,
    pub last_sequence: Option<u64>,
}

#[derive(Debug)]
pub struct DurableGovernance<J> {
    journal: J,
    governance: PromotionGovernance,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableGovernance<J> {
    /// Aligns a journal writer with recovered governance state.
    ///
    /// # Errors
    ///
    /// Rejects writer/recovery sequence disagreement.
    pub fn new(journal: J, recovery: GovernanceRecovery) -> Result<Self, GovernanceStorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(GovernanceStorageError::RecoveryMismatch);
        }
        Ok(Self {
            journal,
            governance: recovery.governance,
            last_sequence: recovery.last_sequence,
            poisoned: None,
        })
    }

    /// Appends and device-syncs before installing one governance transition.
    ///
    /// # Errors
    ///
    /// Returns governance, schema, journal, sequence, or poisoned-owner errors.
    pub fn apply(
        &mut self,
        command: &GovernanceCommand,
    ) -> Result<GovernanceOutcome, GovernanceStorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(GovernanceStorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut preflight = self.governance.clone();
        let result = preflight.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |value| {
            value
                .checked_add(1)
                .ok_or(GovernanceStorageError::SequenceExhausted)
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
            return self.poison(GovernanceStorageError::Journal(error));
        }
        self.last_sequence = Some(sequence);
        if let Err(error) = self.journal.sync_events() {
            return self.poison(GovernanceStorageError::Journal(error));
        }
        self.governance = preflight;
        result.map_err(GovernanceStorageError::Governance)
    }

    #[must_use]
    pub const fn governance(&self) -> &PromotionGovernance {
        &self.governance
    }

    fn poison<T>(&mut self, error: GovernanceStorageError) -> Result<T, GovernanceStorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Strictly replays a segmented promotion-governance journal.
///
/// # Errors
///
/// Rejects corruption, gaps, invalid envelopes, post-halt events, and
/// checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    policy: GovernancePolicy,
    checkpoint: Option<GovernanceCheckpoint>,
) -> Result<GovernanceRecovery, GovernanceStorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut governance = PromotionGovernance::new(policy)?;
    let mut expected = 0_u64;
    let mut last_sequence = None;
    let mut checkpoint_verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(envelope) = reader.next_event()? {
        if halted {
            return Err(GovernanceStorageError::PostHaltEvent);
        }
        if envelope.sequence != expected {
            return Err(GovernanceStorageError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(GovernanceStorageError::SequenceExhausted)?;
        let command = validate_envelope(&envelope)?;
        if let Err(error) = governance.apply(&command) {
            if !governance.is_halted() {
                return Err(GovernanceStorageError::Governance(error));
            }
            halted = true;
        }
        last_sequence = Some(envelope.sequence);
        if checkpoint.is_some_and(|value| value.sequence == envelope.sequence) {
            if governance.snapshot().digest
                != checkpoint
                    .map(|value| value.governance_digest)
                    .ok_or(GovernanceStorageError::CheckpointMismatch)?
            {
                return Err(GovernanceStorageError::CheckpointMismatch);
            }
            checkpoint_verified = true;
        }
    }
    if !checkpoint_verified {
        return Err(GovernanceStorageError::CheckpointSequenceMissing);
    }
    Ok(GovernanceRecovery {
        governance,
        last_sequence,
    })
}

fn validate_envelope(
    envelope: &EventEnvelope,
) -> Result<GovernanceCommand, GovernanceStorageError> {
    if envelope.source != EventSource::System || envelope.market_id != STREAM_ID {
        return Err(GovernanceStorageError::EnvelopeIdentity);
    }
    let command = decode_command(&envelope.payload)?;
    if envelope.event_time_ns != command.recorded_at_ns()
        || envelope.received_time_ns != command.recorded_at_ns()
    {
        return Err(GovernanceStorageError::EnvelopeTimestamp);
    }
    Ok(command)
}

/// Creates and syncs one governance checkpoint without replacement.
///
/// # Errors
///
/// Returns I/O errors, including an existing target.
pub fn write_checkpoint_create_new(
    path: impl AsRef<Path>,
    checkpoint: GovernanceCheckpoint,
) -> Result<(), GovernanceStorageError> {
    let bytes = encode_checkpoint(checkpoint);
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Reads and verifies one governance checkpoint.
///
/// # Errors
///
/// Rejects length, magic, version, reserved bytes, checksum, and I/O failures.
pub fn read_checkpoint(
    path: impl AsRef<Path>,
) -> Result<GovernanceCheckpoint, GovernanceStorageError> {
    decode_checkpoint(&fs::read(path)?)
}

fn encode_checkpoint(checkpoint: GovernanceCheckpoint) -> [u8; CHECKPOINT_BYTES] {
    let mut bytes = [0_u8; CHECKPOINT_BYTES];
    bytes[0..8].copy_from_slice(CHECKPOINT_MAGIC);
    bytes[8..10].copy_from_slice(&CHECKPOINT_VERSION.to_le_bytes());
    bytes[16..24].copy_from_slice(&checkpoint.sequence.to_le_bytes());
    bytes[24..56].copy_from_slice(&checkpoint.governance_digest);
    let checksum = blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]);
    bytes[CHECKPOINT_BODY_BYTES..].copy_from_slice(checksum.as_bytes());
    bytes
}

fn decode_checkpoint(bytes: &[u8]) -> Result<GovernanceCheckpoint, GovernanceStorageError> {
    if bytes.len() != CHECKPOINT_BYTES {
        return Err(GovernanceStorageError::CheckpointLength);
    }
    if bytes.get(0..8) != Some(CHECKPOINT_MAGIC) {
        return Err(GovernanceStorageError::CheckpointMagic);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| GovernanceStorageError::CheckpointLength)?,
    );
    if version != CHECKPOINT_VERSION {
        return Err(GovernanceStorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(GovernanceStorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]).as_bytes() != &bytes[CHECKPOINT_BODY_BYTES..] {
        return Err(GovernanceStorageError::CheckpointChecksum);
    }
    Ok(GovernanceCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| GovernanceStorageError::CheckpointLength)?,
        ),
        governance_digest: bytes[24..56]
            .try_into()
            .map_err(|_| GovernanceStorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum GovernanceStorageError {
    #[error("promotion-governance error: {0}")]
    Governance(#[from] GovernanceError),
    #[error("promotion-governance envelope error: {0:?}")]
    Schema(#[from] SchemaError),
    #[error("promotion-governance journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("promotion-governance segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("promotion-governance I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("promotion-governance sequence mismatch: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("promotion-governance sequence is exhausted")]
    SequenceExhausted,
    #[error("promotion-governance envelope identity is invalid")]
    EnvelopeIdentity,
    #[error("promotion-governance envelope timestamp is invalid")]
    EnvelopeTimestamp,
    #[error("promotion-governance recovery does not match its journal writer")]
    RecoveryMismatch,
    #[error("promotion-governance owner is halted: {0}")]
    Halted(String),
    #[error("promotion-governance checkpoint length is invalid")]
    CheckpointLength,
    #[error("promotion-governance checkpoint magic is invalid")]
    CheckpointMagic,
    #[error("unsupported promotion-governance checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("promotion-governance checkpoint reserved bytes are non-zero")]
    CheckpointReserved,
    #[error("promotion-governance checkpoint checksum is invalid")]
    CheckpointChecksum,
    #[error("promotion-governance checkpoint digest does not match replay")]
    CheckpointMismatch,
    #[error("promotion-governance checkpoint sequence was not present")]
    CheckpointSequenceMissing,
    #[error("promotion-governance journal contains an event after terminal halt")]
    PostHaltEvent,
}
