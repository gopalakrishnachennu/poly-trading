use crate::{
    decode_command, encode_command, ApplyOutcome, Error as DomainError, ReconcilerConfig,
    ReconciliationCommand, SettlementReconciler,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

const STREAM_ID: &str = "__settlement_reconciliation__";
const CHECKPOINT_MAGIC: &[u8; 8] = b"POLYSRP1";
const CHECKPOINT_VERSION: u16 = 1;
const CHECKPOINT_BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ReconciliationCheckpoint {
    pub sequence: u64,
    pub reconciler_digest: [u8; 32],
}

#[derive(Debug)]
pub struct ReconciliationRecovery {
    pub reconciler: SettlementReconciler,
    pub last_sequence: Option<u64>,
}

/// Journal-first single owner for reconciliation commands.
#[derive(Debug)]
pub struct DurableReconciler<J> {
    journal: J,
    reconciler: SettlementReconciler,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableReconciler<J> {
    /// Aligns an opened writer with recovered state.
    ///
    /// # Errors
    ///
    /// Rejects writer/recovery sequence disagreement.
    pub fn new(journal: J, recovery: ReconciliationRecovery) -> Result<Self, StorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(StorageError::RecoveryMismatch);
        }
        Ok(Self {
            journal,
            reconciler: recovery.reconciler,
            last_sequence: recovery.last_sequence,
            poisoned: None,
        })
    }

    /// Preflights, appends, device-syncs, and only then installs the transition.
    /// Integrity-failure commands are durable before their absorbing halt is
    /// exposed.
    ///
    /// # Errors
    ///
    /// Returns domain, journal, schema, or sequence errors. Failures after
    /// append poison this live owner.
    pub fn apply(&mut self, command: &ReconciliationCommand) -> Result<ApplyOutcome, StorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(StorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut preflight = self.reconciler.clone();
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
        self.reconciler = preflight;
        result.map_err(StorageError::Domain)
    }

    /// Synchronizes the current durable prefix.
    ///
    /// # Errors
    ///
    /// A sync failure poisons the live owner.
    pub fn sync(&mut self) -> Result<(), StorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(StorageError::Halted(reason.clone()));
        }
        self.journal
            .sync_events()
            .map_err(StorageError::Journal)
            .or_else(|error| self.poison(error))
    }

    #[must_use]
    pub const fn reconciler(&self) -> &SettlementReconciler {
        &self.reconciler
    }

    #[must_use]
    pub const fn last_sequence(&self) -> Option<u64> {
        self.last_sequence
    }

    fn poison<T>(&mut self, error: StorageError) -> Result<T, StorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Strictly replays a complete segmented reconciliation journal.
///
/// # Errors
///
/// Rejects corruption, gaps, invalid envelopes, post-halt events, domain
/// inconsistencies, and checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    config: ReconcilerConfig,
    checkpoint: Option<ReconciliationCheckpoint>,
) -> Result<ReconciliationRecovery, StorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut reconciler = SettlementReconciler::new(config)?;
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
        if reconciler.apply(&command).is_err() {
            if !reconciler.is_halted() {
                return Err(StorageError::Domain(DomainError::Config));
            }
            halted = true;
        }
        last_sequence = Some(envelope.sequence);
        if checkpoint.is_some_and(|value| value.sequence == envelope.sequence) {
            let expected_digest = checkpoint
                .map(|value| value.reconciler_digest)
                .ok_or(StorageError::CheckpointMismatch)?;
            if reconciler.snapshot().digest != expected_digest {
                return Err(StorageError::CheckpointMismatch);
            }
            checkpoint_verified = true;
        }
    }
    if !checkpoint_verified {
        return Err(StorageError::CheckpointSequenceMissing);
    }
    Ok(ReconciliationRecovery {
        reconciler,
        last_sequence,
    })
}

fn validate_envelope(envelope: &EventEnvelope) -> Result<ReconciliationCommand, StorageError> {
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

/// Creates and device-syncs a new checkpoint without replacement.
///
/// # Errors
///
/// Returns I/O errors, including existing targets.
pub fn write_checkpoint_create_new(
    path: impl AsRef<Path>,
    checkpoint: ReconciliationCheckpoint,
) -> Result<(), StorageError> {
    let bytes = encode_checkpoint(checkpoint);
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Reads and verifies one exact checkpoint.
///
/// # Errors
///
/// Rejects length, magic, version, reserved-byte, checksum, and I/O failures.
pub fn read_checkpoint(path: impl AsRef<Path>) -> Result<ReconciliationCheckpoint, StorageError> {
    decode_checkpoint(&fs::read(path)?)
}

fn encode_checkpoint(checkpoint: ReconciliationCheckpoint) -> [u8; CHECKPOINT_BYTES] {
    let mut bytes = [0_u8; CHECKPOINT_BYTES];
    bytes[0..8].copy_from_slice(CHECKPOINT_MAGIC);
    bytes[8..10].copy_from_slice(&CHECKPOINT_VERSION.to_le_bytes());
    bytes[16..24].copy_from_slice(&checkpoint.sequence.to_le_bytes());
    bytes[24..56].copy_from_slice(&checkpoint.reconciler_digest);
    let checksum = blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]);
    bytes[CHECKPOINT_BODY_BYTES..].copy_from_slice(checksum.as_bytes());
    bytes
}

fn decode_checkpoint(bytes: &[u8]) -> Result<ReconciliationCheckpoint, StorageError> {
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
    Ok(ReconciliationCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| StorageError::CheckpointLength)?,
        ),
        reconciler_digest: bytes[24..56]
            .try_into()
            .map_err(|_| StorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("settlement reconciliation error: {0}")]
    Domain(#[from] DomainError),
    #[error("reconciliation envelope error: {0:?}")]
    Schema(#[from] SchemaError),
    #[error("reconciliation journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("reconciliation segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("reconciliation I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("reconciliation sequence mismatch: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("reconciliation sequence is exhausted")]
    SequenceExhausted,
    #[error("reconciliation envelope source or identity is invalid")]
    EnvelopeIdentity,
    #[error("reconciliation envelope timestamp does not match its command")]
    EnvelopeTimestamp,
    #[error("journal writer and recovered reconciler disagree")]
    RecoveryMismatch,
    #[error("durable event follows an absorbing reconciliation halt")]
    PostHaltEvent,
    #[error("reconciliation checkpoint digest does not match its prefix")]
    CheckpointMismatch,
    #[error("reconciliation checkpoint sequence is absent from the journal")]
    CheckpointSequenceMissing,
    #[error("reconciliation checkpoint length is invalid")]
    CheckpointLength,
    #[error("reconciliation checkpoint magic is invalid")]
    CheckpointMagic,
    #[error("unsupported reconciliation checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("reconciliation checkpoint reserved bytes are non-zero")]
    CheckpointReserved,
    #[error("reconciliation checkpoint checksum mismatch")]
    CheckpointChecksum,
    #[error("durable reconciler is halted: {0}")]
    Halted(String),
}
