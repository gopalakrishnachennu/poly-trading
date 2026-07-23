use crate::{
    decode_command, encode_command, ChangeCommand, ChangeControlPolicy, ChangeOutcome,
    DeploymentChangeControl, Error as ChangeError,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

const STREAM_ID: &str = "__deployment_change_control__";
const CHECKPOINT_MAGIC: &[u8; 8] = b"POLYDHC1";
const CHECKPOINT_VERSION: u16 = 1;
const CHECKPOINT_BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChangeControlCheckpoint {
    pub sequence: u64,
    pub change_control_digest: [u8; 32],
}

#[derive(Debug)]
pub struct ChangeControlRecovery {
    pub change_control: DeploymentChangeControl,
    pub last_sequence: Option<u64>,
}

#[derive(Debug)]
pub struct DurableChangeControl<J> {
    journal: J,
    change_control: DeploymentChangeControl,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableChangeControl<J> {
    /// Aligns a journal writer with recovered change-control state.
    ///
    /// # Errors
    ///
    /// Rejects writer/recovery sequence disagreement.
    pub fn new(
        journal: J,
        recovery: ChangeControlRecovery,
    ) -> Result<Self, ChangeControlStorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(ChangeControlStorageError::RecoveryMismatch);
        }
        Ok(Self {
            journal,
            change_control: recovery.change_control,
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
        command: &ChangeCommand,
    ) -> Result<ChangeOutcome, ChangeControlStorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(ChangeControlStorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut owner = self.change_control.clone();
        let result = owner.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |value| {
            value
                .checked_add(1)
                .ok_or(ChangeControlStorageError::SequenceExhausted)
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
            return self.poison(ChangeControlStorageError::Journal(error));
        }
        self.last_sequence = Some(sequence);
        if let Err(error) = self.journal.sync_events() {
            return self.poison(ChangeControlStorageError::Journal(error));
        }
        self.change_control = owner;
        result.map_err(ChangeControlStorageError::ChangeControl)
    }

    #[must_use]
    pub const fn change_control(&self) -> &DeploymentChangeControl {
        &self.change_control
    }

    fn poison<T>(
        &mut self,
        error: ChangeControlStorageError,
    ) -> Result<T, ChangeControlStorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Strictly replays one segmented change-control journal.
///
/// # Errors
///
/// Rejects corruption, gaps, invalid envelopes, post-halt events and checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    policy: ChangeControlPolicy,
    checkpoint: Option<ChangeControlCheckpoint>,
) -> Result<ChangeControlRecovery, ChangeControlStorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut owner = DeploymentChangeControl::new(policy)?;
    let mut expected = 0_u64;
    let mut last_sequence = None;
    let mut checkpoint_verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(envelope) = reader.next_event()? {
        if halted {
            return Err(ChangeControlStorageError::PostHaltEvent);
        }
        if envelope.sequence != expected {
            return Err(ChangeControlStorageError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(ChangeControlStorageError::SequenceExhausted)?;
        let command = validate_envelope(&envelope)?;
        if let Err(error) = owner.apply(&command) {
            if !owner.is_halted() {
                return Err(ChangeControlStorageError::ChangeControl(error));
            }
            halted = true;
        }
        last_sequence = Some(envelope.sequence);
        if checkpoint.is_some_and(|value| value.sequence == envelope.sequence) {
            if owner.snapshot().digest
                != checkpoint
                    .map(|value| value.change_control_digest)
                    .ok_or(ChangeControlStorageError::CheckpointMismatch)?
            {
                return Err(ChangeControlStorageError::CheckpointMismatch);
            }
            checkpoint_verified = true;
        }
    }
    if !checkpoint_verified {
        return Err(ChangeControlStorageError::CheckpointSequenceMissing);
    }
    Ok(ChangeControlRecovery {
        change_control: owner,
        last_sequence,
    })
}

fn validate_envelope(envelope: &EventEnvelope) -> Result<ChangeCommand, ChangeControlStorageError> {
    if envelope.source != EventSource::System || envelope.market_id != STREAM_ID {
        return Err(ChangeControlStorageError::EnvelopeIdentity);
    }
    let command = decode_command(&envelope.payload)?;
    if envelope.event_time_ns != command.recorded_at_ns()
        || envelope.received_time_ns != command.recorded_at_ns()
    {
        return Err(ChangeControlStorageError::EnvelopeTimestamp);
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
    checkpoint: ChangeControlCheckpoint,
) -> Result<(), ChangeControlStorageError> {
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
pub fn read_checkpoint(
    path: impl AsRef<Path>,
) -> Result<ChangeControlCheckpoint, ChangeControlStorageError> {
    decode_checkpoint(&fs::read(path)?)
}

fn encode_checkpoint(checkpoint: ChangeControlCheckpoint) -> [u8; CHECKPOINT_BYTES] {
    let mut bytes = [0_u8; CHECKPOINT_BYTES];
    bytes[0..8].copy_from_slice(CHECKPOINT_MAGIC);
    bytes[8..10].copy_from_slice(&CHECKPOINT_VERSION.to_le_bytes());
    bytes[16..24].copy_from_slice(&checkpoint.sequence.to_le_bytes());
    bytes[24..56].copy_from_slice(&checkpoint.change_control_digest);
    let checksum = blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]);
    bytes[CHECKPOINT_BODY_BYTES..].copy_from_slice(checksum.as_bytes());
    bytes
}

fn decode_checkpoint(bytes: &[u8]) -> Result<ChangeControlCheckpoint, ChangeControlStorageError> {
    if bytes.len() != CHECKPOINT_BYTES {
        return Err(ChangeControlStorageError::CheckpointLength);
    }
    if bytes.get(0..8) != Some(CHECKPOINT_MAGIC) {
        return Err(ChangeControlStorageError::CheckpointMagic);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| ChangeControlStorageError::CheckpointLength)?,
    );
    if version != CHECKPOINT_VERSION {
        return Err(ChangeControlStorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(ChangeControlStorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]).as_bytes() != &bytes[CHECKPOINT_BODY_BYTES..] {
        return Err(ChangeControlStorageError::CheckpointChecksum);
    }
    Ok(ChangeControlCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| ChangeControlStorageError::CheckpointLength)?,
        ),
        change_control_digest: bytes[24..56]
            .try_into()
            .map_err(|_| ChangeControlStorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum ChangeControlStorageError {
    #[error("deployment change-control error: {0}")]
    ChangeControl(#[from] ChangeError),
    #[error("deployment change-control envelope error: {0:?}")]
    Schema(#[from] SchemaError),
    #[error("deployment change-control journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("deployment change-control segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("deployment change-control I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("deployment change-control sequence mismatch: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("deployment change-control sequence is exhausted")]
    SequenceExhausted,
    #[error("deployment change-control envelope identity is invalid")]
    EnvelopeIdentity,
    #[error("deployment change-control envelope timestamp is invalid")]
    EnvelopeTimestamp,
    #[error("deployment change-control recovery does not match its journal writer")]
    RecoveryMismatch,
    #[error("deployment change-control durable owner is halted: {0}")]
    Halted(String),
    #[error("deployment change-control checkpoint length is invalid")]
    CheckpointLength,
    #[error("deployment change-control checkpoint magic is invalid")]
    CheckpointMagic,
    #[error("unsupported deployment change-control checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("deployment change-control checkpoint reserved bytes are non-zero")]
    CheckpointReserved,
    #[error("deployment change-control checkpoint checksum is invalid")]
    CheckpointChecksum,
    #[error("deployment change-control checkpoint digest does not match replay")]
    CheckpointMismatch,
    #[error("deployment change-control checkpoint sequence was not present")]
    CheckpointSequenceMissing,
    #[error("deployment change-control journal contains an event after terminal halt")]
    PostHaltEvent,
}
