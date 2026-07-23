use crate::{
    decode_command, encode_command, CertificationCommand, CertificationOutcome,
    CertificationPolicy, DeploymentAdapterCertification, Error as CertificationError,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

const STREAM_ID: &str = "__deployment_adapter_certification__";
const CHECKPOINT_MAGIC: &[u8; 8] = b"POLYDCC1";
const CHECKPOINT_VERSION: u16 = 1;
const CHECKPOINT_BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CertificationCheckpoint {
    pub sequence: u64,
    pub certification_digest: [u8; 32],
}

#[derive(Debug)]
pub struct CertificationRecovery {
    pub certification: DeploymentAdapterCertification,
    pub last_sequence: Option<u64>,
}

#[derive(Debug)]
pub struct DurableCertification<J> {
    journal: J,
    certification: DeploymentAdapterCertification,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableCertification<J> {
    /// Aligns a journal writer with recovered certification state.
    ///
    /// # Errors
    ///
    /// Rejects writer/recovery sequence disagreement.
    pub fn new(
        journal: J,
        recovery: CertificationRecovery,
    ) -> Result<Self, CertificationStorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(CertificationStorageError::RecoveryMismatch);
        }
        Ok(Self {
            journal,
            certification: recovery.certification,
            last_sequence: recovery.last_sequence,
            poisoned: None,
        })
    }

    /// Journals and device-syncs before installing one transition.
    ///
    /// # Errors
    ///
    /// Returns certification, schema, journal, sequence, or poisoned-owner errors.
    pub fn apply(
        &mut self,
        command: &CertificationCommand,
    ) -> Result<CertificationOutcome, CertificationStorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(CertificationStorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut certification = self.certification.clone();
        let result = certification.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |value| {
            value
                .checked_add(1)
                .ok_or(CertificationStorageError::SequenceExhausted)
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
            return self.poison(CertificationStorageError::Journal(error));
        }
        self.last_sequence = Some(sequence);
        if let Err(error) = self.journal.sync_events() {
            return self.poison(CertificationStorageError::Journal(error));
        }
        self.certification = certification;
        result.map_err(CertificationStorageError::Certification)
    }

    #[must_use]
    pub const fn certification(&self) -> &DeploymentAdapterCertification {
        &self.certification
    }

    fn poison<T>(
        &mut self,
        error: CertificationStorageError,
    ) -> Result<T, CertificationStorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Strictly replays one segmented certification journal.
///
/// # Errors
///
/// Rejects corruption, gaps, invalid envelopes, post-halt events and checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    policy: CertificationPolicy,
    checkpoint: Option<CertificationCheckpoint>,
) -> Result<CertificationRecovery, CertificationStorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut certification = DeploymentAdapterCertification::new(policy)?;
    let mut expected = 0_u64;
    let mut last_sequence = None;
    let mut checkpoint_verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(envelope) = reader.next_event()? {
        if halted {
            return Err(CertificationStorageError::PostHaltEvent);
        }
        if envelope.sequence != expected {
            return Err(CertificationStorageError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(CertificationStorageError::SequenceExhausted)?;
        let command = validate_envelope(&envelope)?;
        if let Err(error) = certification.apply(&command) {
            if !certification.is_halted() {
                return Err(CertificationStorageError::Certification(error));
            }
            halted = true;
        }
        last_sequence = Some(envelope.sequence);
        if checkpoint.is_some_and(|value| value.sequence == envelope.sequence) {
            if certification.snapshot().digest
                != checkpoint
                    .map(|value| value.certification_digest)
                    .ok_or(CertificationStorageError::CheckpointMismatch)?
            {
                return Err(CertificationStorageError::CheckpointMismatch);
            }
            checkpoint_verified = true;
        }
    }
    if !checkpoint_verified {
        return Err(CertificationStorageError::CheckpointSequenceMissing);
    }
    Ok(CertificationRecovery {
        certification,
        last_sequence,
    })
}

fn validate_envelope(
    envelope: &EventEnvelope,
) -> Result<CertificationCommand, CertificationStorageError> {
    if envelope.source != EventSource::System || envelope.market_id != STREAM_ID {
        return Err(CertificationStorageError::EnvelopeIdentity);
    }
    let command = decode_command(&envelope.payload)?;
    if envelope.event_time_ns != command.recorded_at_ns()
        || envelope.received_time_ns != command.recorded_at_ns()
    {
        return Err(CertificationStorageError::EnvelopeTimestamp);
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
    checkpoint: CertificationCheckpoint,
) -> Result<(), CertificationStorageError> {
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
) -> Result<CertificationCheckpoint, CertificationStorageError> {
    decode_checkpoint(&fs::read(path)?)
}

fn encode_checkpoint(checkpoint: CertificationCheckpoint) -> [u8; CHECKPOINT_BYTES] {
    let mut bytes = [0_u8; CHECKPOINT_BYTES];
    bytes[0..8].copy_from_slice(CHECKPOINT_MAGIC);
    bytes[8..10].copy_from_slice(&CHECKPOINT_VERSION.to_le_bytes());
    bytes[16..24].copy_from_slice(&checkpoint.sequence.to_le_bytes());
    bytes[24..56].copy_from_slice(&checkpoint.certification_digest);
    let checksum = blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]);
    bytes[CHECKPOINT_BODY_BYTES..].copy_from_slice(checksum.as_bytes());
    bytes
}

fn decode_checkpoint(bytes: &[u8]) -> Result<CertificationCheckpoint, CertificationStorageError> {
    if bytes.len() != CHECKPOINT_BYTES {
        return Err(CertificationStorageError::CheckpointLength);
    }
    if bytes.get(0..8) != Some(CHECKPOINT_MAGIC) {
        return Err(CertificationStorageError::CheckpointMagic);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| CertificationStorageError::CheckpointLength)?,
    );
    if version != CHECKPOINT_VERSION {
        return Err(CertificationStorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(CertificationStorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]).as_bytes() != &bytes[CHECKPOINT_BODY_BYTES..] {
        return Err(CertificationStorageError::CheckpointChecksum);
    }
    Ok(CertificationCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| CertificationStorageError::CheckpointLength)?,
        ),
        certification_digest: bytes[24..56]
            .try_into()
            .map_err(|_| CertificationStorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum CertificationStorageError {
    #[error("deployment-adapter certification error: {0}")]
    Certification(#[from] CertificationError),
    #[error("deployment-adapter certification envelope error: {0:?}")]
    Schema(#[from] SchemaError),
    #[error("deployment-adapter certification journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("deployment-adapter certification segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("deployment-adapter certification I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error(
        "deployment-adapter certification sequence mismatch: expected {expected}, got {actual}"
    )]
    Sequence { expected: u64, actual: u64 },
    #[error("deployment-adapter certification sequence is exhausted")]
    SequenceExhausted,
    #[error("deployment-adapter certification envelope identity is invalid")]
    EnvelopeIdentity,
    #[error("deployment-adapter certification envelope timestamp is invalid")]
    EnvelopeTimestamp,
    #[error("deployment-adapter certification recovery does not match its journal writer")]
    RecoveryMismatch,
    #[error("deployment-adapter certification durable owner is halted: {0}")]
    Halted(String),
    #[error("deployment-adapter certification checkpoint length is invalid")]
    CheckpointLength,
    #[error("deployment-adapter certification checkpoint magic is invalid")]
    CheckpointMagic,
    #[error("unsupported deployment-adapter certification checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("deployment-adapter certification checkpoint reserved bytes are non-zero")]
    CheckpointReserved,
    #[error("deployment-adapter certification checkpoint checksum is invalid")]
    CheckpointChecksum,
    #[error("deployment-adapter certification checkpoint digest does not match replay")]
    CheckpointMismatch,
    #[error("deployment-adapter certification checkpoint sequence was not present")]
    CheckpointSequenceMissing,
    #[error("deployment-adapter certification journal contains an event after terminal halt")]
    PostHaltEvent,
}
