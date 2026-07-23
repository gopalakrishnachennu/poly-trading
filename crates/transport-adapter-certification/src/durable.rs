use crate::{
    decode_command, encode_command, Error as CertificationError, TransportAdapterCertification,
    TransportCertificationPolicy, TransportCommand, TransportOutcome,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};
use thiserror::Error;

const STREAM: &str = "__transport_adapter_certification__";
const MAGIC: &[u8; 8] = b"POLYTCC1";
const VERSION: u16 = 1;
const BODY: usize = 56;
const BYTES: usize = 88;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TransportCheckpoint {
    pub sequence: u64,
    pub state_digest: [u8; 32],
}
#[derive(Debug)]
pub struct TransportRecovery {
    pub owner: TransportAdapterCertification,
    pub last_sequence: Option<u64>,
}
#[derive(Debug)]
pub struct DurableTransportCertification<J> {
    journal: J,
    owner: TransportAdapterCertification,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableTransportCertification<J> {
    /// Aligns a journal writer with recovered state.
    ///
    /// # Errors
    ///
    /// Rejects writer and recovery sequence disagreement.
    pub fn new(journal: J, recovery: TransportRecovery) -> Result<Self, TransportStorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(TransportStorageError::RecoveryMismatch);
        }
        Ok(Self {
            journal,
            owner: recovery.owner,
            last_sequence: recovery.last_sequence,
            poisoned: None,
        })
    }
    /// Journals and syncs before installing one transition.
    ///
    /// # Errors
    ///
    /// Returns state, schema, journal, sequence, or poisoned-owner failures.
    pub fn apply(
        &mut self,
        command: &TransportCommand,
    ) -> Result<TransportOutcome, TransportStorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(TransportStorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut next = self.owner.clone();
        let result = next.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |v| {
            v.checked_add(1)
                .ok_or(TransportStorageError::SequenceExhausted)
        })?;
        let at = command.recorded_at_ns();
        let envelope = EventEnvelope::new(
            EventSource::System,
            sequence,
            at,
            at,
            STREAM.to_owned(),
            payload,
        )?;
        if let Err(e) = self.journal.append_event(&envelope) {
            return self.poison(TransportStorageError::Journal(e));
        }
        self.last_sequence = Some(sequence);
        if let Err(e) = self.journal.sync_events() {
            return self.poison(TransportStorageError::Journal(e));
        }
        self.owner = next;
        result.map_err(TransportStorageError::Certification)
    }
    #[must_use]
    pub const fn owner(&self) -> &TransportAdapterCertification {
        &self.owner
    }
    fn poison<T>(&mut self, error: TransportStorageError) -> Result<T, TransportStorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Strictly replays one segmented certification journal.
///
/// # Errors
///
/// Rejects corruption, gaps, invalid envelopes, post-halt events, and checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    policy: TransportCertificationPolicy,
    checkpoint: Option<TransportCheckpoint>,
) -> Result<TransportRecovery, TransportStorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut owner = TransportAdapterCertification::new(policy)?;
    let mut expected = 0;
    let mut last = None;
    let mut verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(envelope) = reader.next_event()? {
        if halted {
            return Err(TransportStorageError::PostHaltEvent);
        }
        if envelope.sequence != expected {
            return Err(TransportStorageError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(TransportStorageError::SequenceExhausted)?;
        if envelope.source != EventSource::System || envelope.market_id != STREAM {
            return Err(TransportStorageError::EnvelopeIdentity);
        }
        let command = decode_command(&envelope.payload)?;
        if envelope.event_time_ns != command.recorded_at_ns()
            || envelope.received_time_ns != command.recorded_at_ns()
        {
            return Err(TransportStorageError::EnvelopeTimestamp);
        }
        if owner.apply(&command).is_err() {
            if !owner.is_halted() {
                return Err(TransportStorageError::Replay);
            }
            halted = true;
        }
        last = Some(envelope.sequence);
        if checkpoint.is_some_and(|v| v.sequence == envelope.sequence) {
            if owner.snapshot().digest
                != checkpoint
                    .ok_or(TransportStorageError::CheckpointMismatch)?
                    .state_digest
            {
                return Err(TransportStorageError::CheckpointMismatch);
            }
            verified = true;
        }
    }
    if !verified {
        return Err(TransportStorageError::CheckpointSequenceMissing);
    }
    Ok(TransportRecovery {
        owner,
        last_sequence: last,
    })
}

/// Creates and syncs one checkpoint without replacement.
///
/// # Errors
///
/// Returns I/O failures, including an existing target.
pub fn write_checkpoint_create_new(
    path: impl AsRef<Path>,
    checkpoint: TransportCheckpoint,
) -> Result<(), TransportStorageError> {
    let mut bytes = [0_u8; BYTES];
    bytes[..8].copy_from_slice(MAGIC);
    bytes[8..10].copy_from_slice(&VERSION.to_le_bytes());
    bytes[16..24].copy_from_slice(&checkpoint.sequence.to_le_bytes());
    bytes[24..56].copy_from_slice(&checkpoint.state_digest);
    let sum = blake3::hash(&bytes[..BODY]);
    bytes[BODY..].copy_from_slice(sum.as_bytes());
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
) -> Result<TransportCheckpoint, TransportStorageError> {
    let bytes = fs::read(path)?;
    if bytes.len() != BYTES || bytes.get(..8) != Some(MAGIC) {
        return Err(TransportStorageError::CheckpointLength);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| TransportStorageError::CheckpointLength)?,
    );
    if version != VERSION {
        return Err(TransportStorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(TransportStorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..BODY]).as_bytes() != &bytes[BODY..] {
        return Err(TransportStorageError::CheckpointChecksum);
    }
    Ok(TransportCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| TransportStorageError::CheckpointLength)?,
        ),
        state_digest: bytes[24..56]
            .try_into()
            .map_err(|_| TransportStorageError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum TransportStorageError {
    #[error("transport certification error: {0}")]
    Certification(#[from] CertificationError),
    #[error("transport envelope error: {0:?}")]
    Schema(#[from] SchemaError),
    #[error("transport journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("transport segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("transport checkpoint I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("transport recovery mismatch")]
    RecoveryMismatch,
    #[error("transport sequence expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("transport sequence exhausted")]
    SequenceExhausted,
    #[error("transport envelope identity invalid")]
    EnvelopeIdentity,
    #[error("transport envelope timestamp invalid")]
    EnvelopeTimestamp,
    #[error("transport replay failed without halt")]
    Replay,
    #[error("transport post-halt event exists")]
    PostHaltEvent,
    #[error("transport checkpoint length invalid")]
    CheckpointLength,
    #[error("unsupported transport checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("transport checkpoint reserved bytes are non-zero")]
    CheckpointReserved,
    #[error("transport checkpoint checksum invalid")]
    CheckpointChecksum,
    #[error("transport checkpoint mismatch")]
    CheckpointMismatch,
    #[error("transport checkpoint sequence missing")]
    CheckpointSequenceMissing,
    #[error("transport durable owner halted: {0}")]
    Halted(String),
}
