use crate::{
    decode_command, encode_command, DeploymentOrchestrator, Error as OrchestrationError,
    OrchestrationCommand, OrchestrationOutcome, OrchestrationPolicy,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

const STREAM_ID: &str = "__deployment_orchestration_simulator__";
const CHECKPOINT_MAGIC: &[u8; 8] = b"POLYDOC1";
const CHECKPOINT_VERSION: u16 = 1;
const CHECKPOINT_BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct OrchestrationCheckpoint {
    pub sequence: u64,
    pub orchestrator_digest: [u8; 32],
}

#[derive(Debug)]
pub struct OrchestrationRecovery {
    pub orchestrator: DeploymentOrchestrator,
    pub last_sequence: Option<u64>,
}

#[derive(Debug)]
pub struct DurableOrchestrator<J> {
    journal: J,
    orchestrator: DeploymentOrchestrator,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableOrchestrator<J> {
    /// Aligns a journal writer with recovered orchestration state.
    ///
    /// # Errors
    ///
    /// Rejects writer/recovery sequence disagreement.
    pub fn new(
        journal: J,
        recovery: OrchestrationRecovery,
    ) -> Result<Self, OrchestrationStorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(OrchestrationStorageError::RecoveryMismatch);
        }
        Ok(Self {
            journal,
            orchestrator: recovery.orchestrator,
            last_sequence: recovery.last_sequence,
            poisoned: None,
        })
    }

    /// Journals and device-syncs before installing one transition.
    ///
    /// # Errors
    ///
    /// Returns orchestration, schema, journal, sequence, or poisoned-owner errors.
    pub fn apply(
        &mut self,
        command: &OrchestrationCommand,
    ) -> Result<OrchestrationOutcome, OrchestrationStorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(OrchestrationStorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut orchestrator = self.orchestrator.clone();
        let result = orchestrator.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |value| {
            value
                .checked_add(1)
                .ok_or(OrchestrationStorageError::SequenceExhausted)
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
            return self.poison(OrchestrationStorageError::Journal(error));
        }
        self.last_sequence = Some(sequence);
        if let Err(error) = self.journal.sync_events() {
            return self.poison(OrchestrationStorageError::Journal(error));
        }
        self.orchestrator = orchestrator;
        result.map_err(OrchestrationStorageError::Orchestration)
    }

    #[must_use]
    pub const fn orchestrator(&self) -> &DeploymentOrchestrator {
        &self.orchestrator
    }

    fn poison<T>(
        &mut self,
        error: OrchestrationStorageError,
    ) -> Result<T, OrchestrationStorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Strictly replays one segmented orchestration journal.
///
/// # Errors
///
/// Rejects corruption, gaps, invalid envelopes, post-halt events, and checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    policy: OrchestrationPolicy,
    checkpoint: Option<OrchestrationCheckpoint>,
) -> Result<OrchestrationRecovery, OrchestrationStorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut orchestrator = DeploymentOrchestrator::new(policy)?;
    let mut expected = 0_u64;
    let mut last_sequence = None;
    let mut checkpoint_verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(envelope) = reader.next_event()? {
        if halted {
            return Err(OrchestrationStorageError::PostHaltEvent);
        }
        if envelope.sequence != expected {
            return Err(OrchestrationStorageError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(OrchestrationStorageError::SequenceExhausted)?;
        let command = validate_envelope(&envelope)?;
        if let Err(error) = orchestrator.apply(&command) {
            if !orchestrator.is_halted() {
                return Err(OrchestrationStorageError::Orchestration(error));
            }
            halted = true;
        }
        last_sequence = Some(envelope.sequence);
        if checkpoint.is_some_and(|value| value.sequence == envelope.sequence) {
            if orchestrator.snapshot().digest
                != checkpoint
                    .map(|value| value.orchestrator_digest)
                    .ok_or(OrchestrationStorageError::CheckpointMismatch)?
            {
                return Err(OrchestrationStorageError::CheckpointMismatch);
            }
            checkpoint_verified = true;
        }
    }
    if !checkpoint_verified {
        return Err(OrchestrationStorageError::CheckpointSequenceMissing);
    }
    Ok(OrchestrationRecovery {
        orchestrator,
        last_sequence,
    })
}

fn validate_envelope(
    envelope: &EventEnvelope,
) -> Result<OrchestrationCommand, OrchestrationStorageError> {
    if envelope.source != EventSource::System || envelope.market_id != STREAM_ID {
        return Err(OrchestrationStorageError::EnvelopeIdentity);
    }
    let command = decode_command(&envelope.payload)?;
    if envelope.event_time_ns != command.recorded_at_ns()
        || envelope.received_time_ns != command.recorded_at_ns()
    {
        return Err(OrchestrationStorageError::EnvelopeTimestamp);
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
    checkpoint: OrchestrationCheckpoint,
) -> Result<(), OrchestrationStorageError> {
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
) -> Result<OrchestrationCheckpoint, OrchestrationStorageError> {
    decode_checkpoint(&fs::read(path)?)
}

fn encode_checkpoint(checkpoint: OrchestrationCheckpoint) -> [u8; CHECKPOINT_BYTES] {
    let mut bytes = [0_u8; CHECKPOINT_BYTES];
    bytes[0..8].copy_from_slice(CHECKPOINT_MAGIC);
    bytes[8..10].copy_from_slice(&CHECKPOINT_VERSION.to_le_bytes());
    bytes[16..24].copy_from_slice(&checkpoint.sequence.to_le_bytes());
    bytes[24..56].copy_from_slice(&checkpoint.orchestrator_digest);
    let checksum = blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]);
    bytes[CHECKPOINT_BODY_BYTES..].copy_from_slice(checksum.as_bytes());
    bytes
}

fn decode_checkpoint(bytes: &[u8]) -> Result<OrchestrationCheckpoint, OrchestrationStorageError> {
    if bytes.len() != CHECKPOINT_BYTES {
        return Err(OrchestrationStorageError::CheckpointLength);
    }
    if bytes.get(0..8) != Some(CHECKPOINT_MAGIC) {
        return Err(OrchestrationStorageError::CheckpointMagic);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| OrchestrationStorageError::CheckpointLength)?,
    );
    if version != CHECKPOINT_VERSION {
        return Err(OrchestrationStorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(OrchestrationStorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]).as_bytes() != &bytes[CHECKPOINT_BODY_BYTES..] {
        return Err(OrchestrationStorageError::CheckpointChecksum);
    }
    Ok(OrchestrationCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| OrchestrationStorageError::CheckpointLength)?,
        ),
        orchestrator_digest: bytes[24..56]
            .try_into()
            .map_err(|_| OrchestrationStorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum OrchestrationStorageError {
    #[error("deployment-orchestration error: {0}")]
    Orchestration(#[from] OrchestrationError),
    #[error("deployment-orchestration envelope error: {0:?}")]
    Schema(#[from] SchemaError),
    #[error("deployment-orchestration journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("deployment-orchestration segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("deployment-orchestration I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("deployment-orchestration sequence mismatch: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("deployment-orchestration sequence is exhausted")]
    SequenceExhausted,
    #[error("deployment-orchestration envelope identity is invalid")]
    EnvelopeIdentity,
    #[error("deployment-orchestration envelope timestamp is invalid")]
    EnvelopeTimestamp,
    #[error("deployment-orchestration recovery does not match its journal writer")]
    RecoveryMismatch,
    #[error("deployment-orchestration owner is halted: {0}")]
    Halted(String),
    #[error("deployment-orchestration checkpoint length is invalid")]
    CheckpointLength,
    #[error("deployment-orchestration checkpoint magic is invalid")]
    CheckpointMagic,
    #[error("unsupported deployment-orchestration checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("deployment-orchestration checkpoint reserved bytes are non-zero")]
    CheckpointReserved,
    #[error("deployment-orchestration checkpoint checksum is invalid")]
    CheckpointChecksum,
    #[error("deployment-orchestration checkpoint digest does not match replay")]
    CheckpointMismatch,
    #[error("deployment-orchestration checkpoint sequence was not present")]
    CheckpointSequenceMissing,
    #[error("deployment-orchestration journal contains an event after terminal halt")]
    PostHaltEvent,
}
