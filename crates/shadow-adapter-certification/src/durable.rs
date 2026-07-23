use crate::{
    decode_command, encode_command, CertificationCommand, CertificationOutcome,
    Error as AuthorityError, ShadowAdapterCertification,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

const STREAM_ID: &str = "__shadow_adapter_certification__";
const CHECKPOINT_MAGIC: &[u8; 8] = b"POLYSAC1";
const CHECKPOINT_VERSION: u16 = 1;
const CHECKPOINT_BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CertificationCheckpoint {
    pub sequence: u64,
    pub authority_digest: [u8; 32],
}

#[derive(Debug)]
pub struct CertificationRecovery {
    pub authority: ShadowAdapterCertification,
    pub last_sequence: Option<u64>,
}

#[derive(Debug)]
pub struct DurableCertificationAuthority<J> {
    journal: J,
    authority: ShadowAdapterCertification,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableCertificationAuthority<J> {
    /// Aligns a journal writer with recovered certification state.
    ///
    /// # Errors
    ///
    /// Rejects writer/recovery sequence disagreement.
    pub fn new(journal: J, recovery: CertificationRecovery) -> Result<Self, StorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(StorageError::RecoveryMismatch);
        }
        Ok(Self {
            journal,
            authority: recovery.authority,
            last_sequence: recovery.last_sequence,
            poisoned: None,
        })
    }

    /// Appends and syncs before installing certification evidence.
    ///
    /// # Errors
    ///
    /// Returns authority, schema, journal, sequence, or poisoned-owner failures.
    pub fn apply(
        &mut self,
        command: &CertificationCommand,
    ) -> Result<CertificationOutcome, StorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(StorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut preflight = self.authority.clone();
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
        self.authority = preflight;
        result.map_err(StorageError::Authority)
    }

    #[must_use]
    pub const fn authority(&self) -> &ShadowAdapterCertification {
        &self.authority
    }

    fn poison<T>(&mut self, error: StorageError) -> Result<T, StorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Strictly replays a segmented certification journal.
///
/// # Errors
///
/// Rejects corruption, gaps, invalid envelopes, post-halt events, and
/// checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    checkpoint: Option<CertificationCheckpoint>,
) -> Result<CertificationRecovery, StorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut authority = ShadowAdapterCertification::default();
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
        if let Err(error) = authority.apply(&command) {
            if !authority.is_halted() {
                return Err(StorageError::Authority(error));
            }
            halted = true;
        }
        last_sequence = Some(envelope.sequence);
        if checkpoint.is_some_and(|value| value.sequence == envelope.sequence) {
            if authority.snapshot().digest
                != checkpoint
                    .map(|value| value.authority_digest)
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
    Ok(CertificationRecovery {
        authority,
        last_sequence,
    })
}

fn validate_envelope(envelope: &EventEnvelope) -> Result<CertificationCommand, StorageError> {
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

/// Creates and syncs a new checkpoint without replacement.
///
/// # Errors
///
/// Returns I/O errors, including an existing target.
pub fn write_checkpoint_create_new(
    path: impl AsRef<Path>,
    checkpoint: CertificationCheckpoint,
) -> Result<(), StorageError> {
    let bytes = encode_checkpoint(checkpoint);
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Reads and verifies one certification checkpoint.
///
/// # Errors
///
/// Rejects length, magic, version, reserved bytes, checksum, and I/O failures.
pub fn read_checkpoint(path: impl AsRef<Path>) -> Result<CertificationCheckpoint, StorageError> {
    decode_checkpoint(&fs::read(path)?)
}

fn encode_checkpoint(checkpoint: CertificationCheckpoint) -> [u8; CHECKPOINT_BYTES] {
    let mut bytes = [0_u8; CHECKPOINT_BYTES];
    bytes[0..8].copy_from_slice(CHECKPOINT_MAGIC);
    bytes[8..10].copy_from_slice(&CHECKPOINT_VERSION.to_le_bytes());
    bytes[16..24].copy_from_slice(&checkpoint.sequence.to_le_bytes());
    bytes[24..56].copy_from_slice(&checkpoint.authority_digest);
    let checksum = blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]);
    bytes[CHECKPOINT_BODY_BYTES..].copy_from_slice(checksum.as_bytes());
    bytes
}

fn decode_checkpoint(bytes: &[u8]) -> Result<CertificationCheckpoint, StorageError> {
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
    Ok(CertificationCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| StorageError::CheckpointLength)?,
        ),
        authority_digest: bytes[24..56]
            .try_into()
            .map_err(|_| StorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum StorageError {
    #[error("certification authority error: {0}")]
    Authority(#[from] AuthorityError),
    #[error("certification envelope error: {0:?}")]
    Schema(#[from] SchemaError),
    #[error("certification journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("certification segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("certification I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("certification sequence mismatch: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("certification sequence is exhausted")]
    SequenceExhausted,
    #[error("certification envelope identity is invalid")]
    EnvelopeIdentity,
    #[error("certification envelope timestamp is invalid")]
    EnvelopeTimestamp,
    #[error("certification recovery does not match the journal writer")]
    RecoveryMismatch,
    #[error("certification owner is halted: {0}")]
    Halted(String),
    #[error("certification checkpoint length is invalid")]
    CheckpointLength,
    #[error("certification checkpoint magic is invalid")]
    CheckpointMagic,
    #[error("unsupported certification checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("certification checkpoint reserved bytes are non-zero")]
    CheckpointReserved,
    #[error("certification checkpoint checksum is invalid")]
    CheckpointChecksum,
    #[error("certification checkpoint digest does not match replay")]
    CheckpointMismatch,
    #[error("certification checkpoint sequence was not present")]
    CheckpointSequenceMissing,
    #[error("certification journal contains an event after terminal halt")]
    PostHaltEvent,
}
