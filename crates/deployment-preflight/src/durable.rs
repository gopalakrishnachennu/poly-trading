use crate::{
    decode_command, encode_command, DeploymentCommand, DeploymentOutcome, DeploymentPolicy,
    DeploymentPreflight, Error as PreflightError,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

const STREAM_ID: &str = "__deployment_preflight__";
const CHECKPOINT_MAGIC: &[u8; 8] = b"POLYDPC1";
const CHECKPOINT_VERSION: u16 = 1;
const CHECKPOINT_BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct DeploymentCheckpoint {
    pub sequence: u64,
    pub preflight_digest: [u8; 32],
}

#[derive(Debug)]
pub struct DeploymentRecovery {
    pub preflight: DeploymentPreflight,
    pub last_sequence: Option<u64>,
}

#[derive(Debug)]
pub struct DurableDeploymentPreflight<J> {
    journal: J,
    preflight: DeploymentPreflight,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableDeploymentPreflight<J> {
    /// Aligns a writer with recovered deployment-preflight state.
    ///
    /// # Errors
    ///
    /// Rejects writer/recovery sequence disagreement.
    pub fn new(journal: J, recovery: DeploymentRecovery) -> Result<Self, DeploymentStorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(DeploymentStorageError::RecoveryMismatch);
        }
        Ok(Self {
            journal,
            preflight: recovery.preflight,
            last_sequence: recovery.last_sequence,
            poisoned: None,
        })
    }

    /// Journals and device-syncs before installing one transition.
    ///
    /// # Errors
    ///
    /// Returns preflight, schema, journal, sequence, or poisoned-owner errors.
    pub fn apply(
        &mut self,
        command: &DeploymentCommand,
    ) -> Result<DeploymentOutcome, DeploymentStorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(DeploymentStorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut preflight = self.preflight.clone();
        let result = preflight.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |value| {
            value
                .checked_add(1)
                .ok_or(DeploymentStorageError::SequenceExhausted)
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
            return self.poison(DeploymentStorageError::Journal(error));
        }
        self.last_sequence = Some(sequence);
        if let Err(error) = self.journal.sync_events() {
            return self.poison(DeploymentStorageError::Journal(error));
        }
        self.preflight = preflight;
        result.map_err(DeploymentStorageError::Preflight)
    }

    #[must_use]
    pub const fn preflight(&self) -> &DeploymentPreflight {
        &self.preflight
    }

    fn poison<T>(&mut self, error: DeploymentStorageError) -> Result<T, DeploymentStorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Strictly replays one segmented deployment-preflight journal.
///
/// # Errors
///
/// Rejects corruption, gaps, invalid envelopes, post-halt events, and
/// checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    policy: DeploymentPolicy,
    checkpoint: Option<DeploymentCheckpoint>,
) -> Result<DeploymentRecovery, DeploymentStorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut preflight = DeploymentPreflight::new(policy)?;
    let mut expected = 0_u64;
    let mut last_sequence = None;
    let mut checkpoint_verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(envelope) = reader.next_event()? {
        if halted {
            return Err(DeploymentStorageError::PostHaltEvent);
        }
        if envelope.sequence != expected {
            return Err(DeploymentStorageError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(DeploymentStorageError::SequenceExhausted)?;
        let command = validate_envelope(&envelope)?;
        if let Err(error) = preflight.apply(&command) {
            if !preflight.is_halted() {
                return Err(DeploymentStorageError::Preflight(error));
            }
            halted = true;
        }
        last_sequence = Some(envelope.sequence);
        if checkpoint.is_some_and(|value| value.sequence == envelope.sequence) {
            if preflight.snapshot().digest
                != checkpoint
                    .map(|value| value.preflight_digest)
                    .ok_or(DeploymentStorageError::CheckpointMismatch)?
            {
                return Err(DeploymentStorageError::CheckpointMismatch);
            }
            checkpoint_verified = true;
        }
    }
    if !checkpoint_verified {
        return Err(DeploymentStorageError::CheckpointSequenceMissing);
    }
    Ok(DeploymentRecovery {
        preflight,
        last_sequence,
    })
}

fn validate_envelope(
    envelope: &EventEnvelope,
) -> Result<DeploymentCommand, DeploymentStorageError> {
    if envelope.source != EventSource::System || envelope.market_id != STREAM_ID {
        return Err(DeploymentStorageError::EnvelopeIdentity);
    }
    let command = decode_command(&envelope.payload)?;
    if envelope.event_time_ns != command.recorded_at_ns()
        || envelope.received_time_ns != command.recorded_at_ns()
    {
        return Err(DeploymentStorageError::EnvelopeTimestamp);
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
    checkpoint: DeploymentCheckpoint,
) -> Result<(), DeploymentStorageError> {
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
/// Rejects length, magic, version, reserved bytes, checksum, and I/O failures.
pub fn read_checkpoint(
    path: impl AsRef<Path>,
) -> Result<DeploymentCheckpoint, DeploymentStorageError> {
    decode_checkpoint(&fs::read(path)?)
}

fn encode_checkpoint(checkpoint: DeploymentCheckpoint) -> [u8; CHECKPOINT_BYTES] {
    let mut bytes = [0_u8; CHECKPOINT_BYTES];
    bytes[0..8].copy_from_slice(CHECKPOINT_MAGIC);
    bytes[8..10].copy_from_slice(&CHECKPOINT_VERSION.to_le_bytes());
    bytes[16..24].copy_from_slice(&checkpoint.sequence.to_le_bytes());
    bytes[24..56].copy_from_slice(&checkpoint.preflight_digest);
    let checksum = blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]);
    bytes[CHECKPOINT_BODY_BYTES..].copy_from_slice(checksum.as_bytes());
    bytes
}

fn decode_checkpoint(bytes: &[u8]) -> Result<DeploymentCheckpoint, DeploymentStorageError> {
    if bytes.len() != CHECKPOINT_BYTES {
        return Err(DeploymentStorageError::CheckpointLength);
    }
    if bytes.get(0..8) != Some(CHECKPOINT_MAGIC) {
        return Err(DeploymentStorageError::CheckpointMagic);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| DeploymentStorageError::CheckpointLength)?,
    );
    if version != CHECKPOINT_VERSION {
        return Err(DeploymentStorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(DeploymentStorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]).as_bytes() != &bytes[CHECKPOINT_BODY_BYTES..] {
        return Err(DeploymentStorageError::CheckpointChecksum);
    }
    Ok(DeploymentCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| DeploymentStorageError::CheckpointLength)?,
        ),
        preflight_digest: bytes[24..56]
            .try_into()
            .map_err(|_| DeploymentStorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum DeploymentStorageError {
    #[error("deployment-preflight error: {0}")]
    Preflight(#[from] PreflightError),
    #[error("deployment-preflight envelope error: {0:?}")]
    Schema(#[from] SchemaError),
    #[error("deployment-preflight journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("deployment-preflight segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("deployment-preflight I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("deployment-preflight sequence mismatch: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("deployment-preflight sequence is exhausted")]
    SequenceExhausted,
    #[error("deployment-preflight envelope identity is invalid")]
    EnvelopeIdentity,
    #[error("deployment-preflight envelope timestamp is invalid")]
    EnvelopeTimestamp,
    #[error("deployment-preflight recovery does not match its journal writer")]
    RecoveryMismatch,
    #[error("deployment-preflight owner is halted: {0}")]
    Halted(String),
    #[error("deployment-preflight checkpoint length is invalid")]
    CheckpointLength,
    #[error("deployment-preflight checkpoint magic is invalid")]
    CheckpointMagic,
    #[error("unsupported deployment-preflight checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("deployment-preflight checkpoint reserved bytes are non-zero")]
    CheckpointReserved,
    #[error("deployment-preflight checkpoint checksum is invalid")]
    CheckpointChecksum,
    #[error("deployment-preflight checkpoint digest does not match replay")]
    CheckpointMismatch,
    #[error("deployment-preflight checkpoint sequence was not present")]
    CheckpointSequenceMissing,
    #[error("deployment-preflight journal contains an event after terminal halt")]
    PostHaltEvent,
}
