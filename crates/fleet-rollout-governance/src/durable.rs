use crate::{
    decode_command, encode_command, Error as FleetError, FleetCommand, FleetOutcome, FleetPolicy,
    FleetRolloutGovernance,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

const STREAM_ID: &str = "__fleet_rollout_governance__";
const CHECKPOINT_MAGIC: &[u8; 8] = b"POLYFGC1";
const CHECKPOINT_VERSION: u16 = 1;
const CHECKPOINT_BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct FleetCheckpoint {
    pub sequence: u64,
    pub governance_digest: [u8; 32],
}

#[derive(Debug)]
pub struct FleetRecovery {
    pub governance: FleetRolloutGovernance,
    pub last_sequence: Option<u64>,
}

#[derive(Debug)]
pub struct DurableFleetGovernance<J> {
    journal: J,
    governance: FleetRolloutGovernance,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableFleetGovernance<J> {
    /// Aligns a journal writer with recovered fleet-governance state.
    ///
    /// # Errors
    ///
    /// Rejects writer/recovery sequence disagreement.
    pub fn new(journal: J, recovery: FleetRecovery) -> Result<Self, FleetStorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(FleetStorageError::RecoveryMismatch);
        }
        Ok(Self {
            journal,
            governance: recovery.governance,
            last_sequence: recovery.last_sequence,
            poisoned: None,
        })
    }

    /// Journals and device-syncs one command before installing its transition.
    ///
    /// # Errors
    ///
    /// Returns governance, schema, journal, sequence, or poisoned-owner errors.
    pub fn apply(&mut self, command: &FleetCommand) -> Result<FleetOutcome, FleetStorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(FleetStorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut preflight = self.governance.clone();
        let result = preflight.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |value| {
            value
                .checked_add(1)
                .ok_or(FleetStorageError::SequenceExhausted)
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
            return self.poison(FleetStorageError::Journal(error));
        }
        self.last_sequence = Some(sequence);
        if let Err(error) = self.journal.sync_events() {
            return self.poison(FleetStorageError::Journal(error));
        }
        self.governance = preflight;
        result.map_err(FleetStorageError::Governance)
    }

    #[must_use]
    pub const fn governance(&self) -> &FleetRolloutGovernance {
        &self.governance
    }

    fn poison<T>(&mut self, error: FleetStorageError) -> Result<T, FleetStorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Strictly replays one segmented fleet-governance journal.
///
/// # Errors
///
/// Rejects corruption, gaps, invalid envelopes, post-halt events, and
/// checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    policy: FleetPolicy,
    checkpoint: Option<FleetCheckpoint>,
) -> Result<FleetRecovery, FleetStorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut governance = FleetRolloutGovernance::new(policy)?;
    let mut expected = 0_u64;
    let mut last_sequence = None;
    let mut checkpoint_verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(envelope) = reader.next_event()? {
        if halted {
            return Err(FleetStorageError::PostHaltEvent);
        }
        if envelope.sequence != expected {
            return Err(FleetStorageError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(FleetStorageError::SequenceExhausted)?;
        let command = validate_envelope(&envelope)?;
        if let Err(error) = governance.apply(&command) {
            if !governance.is_halted() {
                return Err(FleetStorageError::Governance(error));
            }
            halted = true;
        }
        last_sequence = Some(envelope.sequence);
        if checkpoint.is_some_and(|value| value.sequence == envelope.sequence) {
            if governance.snapshot().digest
                != checkpoint
                    .map(|value| value.governance_digest)
                    .ok_or(FleetStorageError::CheckpointMismatch)?
            {
                return Err(FleetStorageError::CheckpointMismatch);
            }
            checkpoint_verified = true;
        }
    }
    if !checkpoint_verified {
        return Err(FleetStorageError::CheckpointSequenceMissing);
    }
    Ok(FleetRecovery {
        governance,
        last_sequence,
    })
}

fn validate_envelope(envelope: &EventEnvelope) -> Result<FleetCommand, FleetStorageError> {
    if envelope.source != EventSource::System || envelope.market_id != STREAM_ID {
        return Err(FleetStorageError::EnvelopeIdentity);
    }
    let command = decode_command(&envelope.payload)?;
    if envelope.event_time_ns != command.recorded_at_ns()
        || envelope.received_time_ns != command.recorded_at_ns()
    {
        return Err(FleetStorageError::EnvelopeTimestamp);
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
    checkpoint: FleetCheckpoint,
) -> Result<(), FleetStorageError> {
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
pub fn read_checkpoint(path: impl AsRef<Path>) -> Result<FleetCheckpoint, FleetStorageError> {
    decode_checkpoint(&fs::read(path)?)
}

fn encode_checkpoint(checkpoint: FleetCheckpoint) -> [u8; CHECKPOINT_BYTES] {
    let mut bytes = [0_u8; CHECKPOINT_BYTES];
    bytes[0..8].copy_from_slice(CHECKPOINT_MAGIC);
    bytes[8..10].copy_from_slice(&CHECKPOINT_VERSION.to_le_bytes());
    bytes[16..24].copy_from_slice(&checkpoint.sequence.to_le_bytes());
    bytes[24..56].copy_from_slice(&checkpoint.governance_digest);
    let checksum = blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]);
    bytes[CHECKPOINT_BODY_BYTES..].copy_from_slice(checksum.as_bytes());
    bytes
}

fn decode_checkpoint(bytes: &[u8]) -> Result<FleetCheckpoint, FleetStorageError> {
    if bytes.len() != CHECKPOINT_BYTES {
        return Err(FleetStorageError::CheckpointLength);
    }
    if bytes.get(0..8) != Some(CHECKPOINT_MAGIC) {
        return Err(FleetStorageError::CheckpointMagic);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| FleetStorageError::CheckpointLength)?,
    );
    if version != CHECKPOINT_VERSION {
        return Err(FleetStorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(FleetStorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]).as_bytes() != &bytes[CHECKPOINT_BODY_BYTES..] {
        return Err(FleetStorageError::CheckpointChecksum);
    }
    Ok(FleetCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| FleetStorageError::CheckpointLength)?,
        ),
        governance_digest: bytes[24..56]
            .try_into()
            .map_err(|_| FleetStorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum FleetStorageError {
    #[error("fleet-governance error: {0}")]
    Governance(#[from] FleetError),
    #[error("fleet-governance envelope error: {0:?}")]
    Schema(#[from] SchemaError),
    #[error("fleet-governance journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("fleet-governance segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("fleet-governance I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("fleet-governance sequence mismatch: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("fleet-governance sequence is exhausted")]
    SequenceExhausted,
    #[error("fleet-governance envelope identity is invalid")]
    EnvelopeIdentity,
    #[error("fleet-governance envelope timestamp is invalid")]
    EnvelopeTimestamp,
    #[error("fleet-governance recovery does not match its journal writer")]
    RecoveryMismatch,
    #[error("fleet-governance owner is halted: {0}")]
    Halted(String),
    #[error("fleet-governance checkpoint length is invalid")]
    CheckpointLength,
    #[error("fleet-governance checkpoint magic is invalid")]
    CheckpointMagic,
    #[error("unsupported fleet-governance checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("fleet-governance checkpoint reserved bytes are non-zero")]
    CheckpointReserved,
    #[error("fleet-governance checkpoint checksum is invalid")]
    CheckpointChecksum,
    #[error("fleet-governance checkpoint digest does not match replay")]
    CheckpointMismatch,
    #[error("fleet-governance checkpoint sequence was not present")]
    CheckpointSequenceMissing,
    #[error("fleet-governance journal contains an event after terminal halt")]
    PostHaltEvent,
}
