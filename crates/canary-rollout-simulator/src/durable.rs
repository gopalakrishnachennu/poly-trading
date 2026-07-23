use crate::{
    decode_command, encode_command, CanaryRolloutSimulator, Error as RolloutError, RolloutCommand,
    RolloutOutcome, SimulatorPolicy,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

const STREAM_ID: &str = "__canary_rollout_simulator__";
const CHECKPOINT_MAGIC: &[u8; 8] = b"POLYCRC1";
const CHECKPOINT_VERSION: u16 = 1;
const CHECKPOINT_BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RolloutCheckpoint {
    pub sequence: u64,
    pub rollout_digest: [u8; 32],
}

#[derive(Debug)]
pub struct RolloutRecovery {
    pub simulator: CanaryRolloutSimulator,
    pub last_sequence: Option<u64>,
}

#[derive(Debug)]
pub struct DurableRolloutSimulator<J> {
    journal: J,
    simulator: CanaryRolloutSimulator,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableRolloutSimulator<J> {
    /// Aligns one journal writer with recovered simulator state.
    ///
    /// # Errors
    ///
    /// Rejects writer/recovery sequence disagreement.
    pub fn new(journal: J, recovery: RolloutRecovery) -> Result<Self, RolloutStorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(RolloutStorageError::RecoveryMismatch);
        }
        Ok(Self {
            journal,
            simulator: recovery.simulator,
            last_sequence: recovery.last_sequence,
            poisoned: None,
        })
    }

    /// Appends and device-syncs before installing one simulated transition.
    ///
    /// # Errors
    ///
    /// Returns lifecycle, schema, journal, sequence, or poisoned-owner failures.
    pub fn apply(
        &mut self,
        command: &RolloutCommand,
    ) -> Result<RolloutOutcome, RolloutStorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(RolloutStorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut preflight = self.simulator.clone();
        let result = preflight.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |value| {
            value
                .checked_add(1)
                .ok_or(RolloutStorageError::SequenceExhausted)
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
            return self.poison(RolloutStorageError::Journal(error));
        }
        self.last_sequence = Some(sequence);
        if let Err(error) = self.journal.sync_events() {
            return self.poison(RolloutStorageError::Journal(error));
        }
        self.simulator = preflight;
        result.map_err(RolloutStorageError::Rollout)
    }

    #[must_use]
    pub const fn simulator(&self) -> &CanaryRolloutSimulator {
        &self.simulator
    }

    fn poison<T>(&mut self, error: RolloutStorageError) -> Result<T, RolloutStorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Strictly replays one segmented rollout journal.
///
/// # Errors
///
/// Rejects corruption, gaps, invalid envelopes, post-halt events, and
/// checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    policy: SimulatorPolicy,
    checkpoint: Option<RolloutCheckpoint>,
) -> Result<RolloutRecovery, RolloutStorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut simulator = CanaryRolloutSimulator::new(policy)?;
    let mut expected = 0_u64;
    let mut last_sequence = None;
    let mut checkpoint_verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(envelope) = reader.next_event()? {
        if halted {
            return Err(RolloutStorageError::PostHaltEvent);
        }
        if envelope.sequence != expected {
            return Err(RolloutStorageError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(RolloutStorageError::SequenceExhausted)?;
        let command = validate_envelope(&envelope)?;
        if let Err(error) = simulator.apply(&command) {
            if !simulator.is_halted() {
                return Err(RolloutStorageError::Rollout(error));
            }
            halted = true;
        }
        last_sequence = Some(envelope.sequence);
        if checkpoint.is_some_and(|value| value.sequence == envelope.sequence) {
            if simulator.snapshot().digest
                != checkpoint
                    .map(|value| value.rollout_digest)
                    .ok_or(RolloutStorageError::CheckpointMismatch)?
            {
                return Err(RolloutStorageError::CheckpointMismatch);
            }
            checkpoint_verified = true;
        }
    }
    if !checkpoint_verified {
        return Err(RolloutStorageError::CheckpointSequenceMissing);
    }
    Ok(RolloutRecovery {
        simulator,
        last_sequence,
    })
}

fn validate_envelope(envelope: &EventEnvelope) -> Result<RolloutCommand, RolloutStorageError> {
    if envelope.source != EventSource::System || envelope.market_id != STREAM_ID {
        return Err(RolloutStorageError::EnvelopeIdentity);
    }
    let command = decode_command(&envelope.payload)?;
    if envelope.event_time_ns != command.recorded_at_ns()
        || envelope.received_time_ns != command.recorded_at_ns()
    {
        return Err(RolloutStorageError::EnvelopeTimestamp);
    }
    Ok(command)
}

/// Creates and syncs one rollout checkpoint without replacement.
///
/// # Errors
///
/// Returns I/O errors, including an existing target.
pub fn write_checkpoint_create_new(
    path: impl AsRef<Path>,
    checkpoint: RolloutCheckpoint,
) -> Result<(), RolloutStorageError> {
    let bytes = encode_checkpoint(checkpoint);
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Reads and verifies one rollout checkpoint.
///
/// # Errors
///
/// Rejects length, magic, version, reserved bytes, checksum, and I/O failures.
pub fn read_checkpoint(path: impl AsRef<Path>) -> Result<RolloutCheckpoint, RolloutStorageError> {
    decode_checkpoint(&fs::read(path)?)
}

fn encode_checkpoint(checkpoint: RolloutCheckpoint) -> [u8; CHECKPOINT_BYTES] {
    let mut bytes = [0_u8; CHECKPOINT_BYTES];
    bytes[0..8].copy_from_slice(CHECKPOINT_MAGIC);
    bytes[8..10].copy_from_slice(&CHECKPOINT_VERSION.to_le_bytes());
    bytes[16..24].copy_from_slice(&checkpoint.sequence.to_le_bytes());
    bytes[24..56].copy_from_slice(&checkpoint.rollout_digest);
    let checksum = blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]);
    bytes[CHECKPOINT_BODY_BYTES..].copy_from_slice(checksum.as_bytes());
    bytes
}

fn decode_checkpoint(bytes: &[u8]) -> Result<RolloutCheckpoint, RolloutStorageError> {
    if bytes.len() != CHECKPOINT_BYTES {
        return Err(RolloutStorageError::CheckpointLength);
    }
    if bytes.get(0..8) != Some(CHECKPOINT_MAGIC) {
        return Err(RolloutStorageError::CheckpointMagic);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| RolloutStorageError::CheckpointLength)?,
    );
    if version != CHECKPOINT_VERSION {
        return Err(RolloutStorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(RolloutStorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]).as_bytes() != &bytes[CHECKPOINT_BODY_BYTES..] {
        return Err(RolloutStorageError::CheckpointChecksum);
    }
    Ok(RolloutCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| RolloutStorageError::CheckpointLength)?,
        ),
        rollout_digest: bytes[24..56]
            .try_into()
            .map_err(|_| RolloutStorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum RolloutStorageError {
    #[error("canary-rollout error: {0}")]
    Rollout(#[from] RolloutError),
    #[error("canary-rollout envelope error: {0:?}")]
    Schema(#[from] SchemaError),
    #[error("canary-rollout journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("canary-rollout segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("canary-rollout I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("canary-rollout sequence mismatch: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("canary-rollout sequence is exhausted")]
    SequenceExhausted,
    #[error("canary-rollout envelope identity is invalid")]
    EnvelopeIdentity,
    #[error("canary-rollout envelope timestamp is invalid")]
    EnvelopeTimestamp,
    #[error("canary-rollout recovery does not match its journal writer")]
    RecoveryMismatch,
    #[error("canary-rollout owner is halted: {0}")]
    Halted(String),
    #[error("canary-rollout checkpoint length is invalid")]
    CheckpointLength,
    #[error("canary-rollout checkpoint magic is invalid")]
    CheckpointMagic,
    #[error("unsupported canary-rollout checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("canary-rollout checkpoint reserved bytes are non-zero")]
    CheckpointReserved,
    #[error("canary-rollout checkpoint checksum is invalid")]
    CheckpointChecksum,
    #[error("canary-rollout checkpoint digest does not match replay")]
    CheckpointMismatch,
    #[error("canary-rollout checkpoint sequence was not present")]
    CheckpointSequenceMissing,
    #[error("canary-rollout journal contains an event after terminal halt")]
    PostHaltEvent,
}
