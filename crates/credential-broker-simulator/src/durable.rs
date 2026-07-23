use crate::{
    decode_command, encode_command, BrokerCommand, BrokerOutcome, BrokerPolicy,
    CredentialBrokerSimulator, Error as BrokerError,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};
use thiserror::Error;

const STREAM: &str = "__credential_broker_simulator__";
const MAGIC: &[u8; 8] = b"POLYCBC1";
const VERSION: u16 = 1;
const BODY: usize = 56;
const BYTES: usize = 88;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct BrokerCheckpoint {
    pub sequence: u64,
    pub state_digest: [u8; 32],
}
#[derive(Debug)]
pub struct BrokerRecovery {
    pub owner: CredentialBrokerSimulator,
    pub last_sequence: Option<u64>,
}
#[derive(Debug)]
pub struct DurableCredentialBroker<J> {
    journal: J,
    owner: CredentialBrokerSimulator,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}
impl<J: EventJournal> DurableCredentialBroker<J> {
    /// Aligns a journal writer with recovered state.
    ///
    /// # Errors
    ///
    /// Rejects writer and recovery sequence disagreement.
    pub fn new(journal: J, recovery: BrokerRecovery) -> Result<Self, BrokerStorageError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(BrokerStorageError::RecoveryMismatch);
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
    pub fn apply(&mut self, command: &BrokerCommand) -> Result<BrokerOutcome, BrokerStorageError> {
        if let Some(reason) = &self.poisoned {
            return Err(BrokerStorageError::Halted(reason.clone()));
        }
        let payload = encode_command(command)?;
        let mut next = self.owner.clone();
        let result = next.apply(command);
        let sequence = self.last_sequence.map_or(Ok(0), |v| {
            v.checked_add(1)
                .ok_or(BrokerStorageError::SequenceExhausted)
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
            return self.poison(BrokerStorageError::Journal(e));
        }
        self.last_sequence = Some(sequence);
        if let Err(e) = self.journal.sync_events() {
            return self.poison(BrokerStorageError::Journal(e));
        }
        self.owner = next;
        result.map_err(BrokerStorageError::Broker)
    }
    #[must_use]
    pub const fn owner(&self) -> &CredentialBrokerSimulator {
        &self.owner
    }
    fn poison<T>(&mut self, error: BrokerStorageError) -> Result<T, BrokerStorageError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}
/// Strictly replays one segmented broker journal.
///
/// # Errors
///
/// Rejects corruption, gaps, invalid envelopes, post-halt events, and checkpoint mismatch.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    policy: BrokerPolicy,
    checkpoint: Option<BrokerCheckpoint>,
) -> Result<BrokerRecovery, BrokerStorageError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut owner = CredentialBrokerSimulator::new(policy)?;
    let mut expected = 0;
    let mut last = None;
    let mut verified = checkpoint.is_none();
    let mut halted = false;
    while let Some(envelope) = reader.next_event()? {
        if halted {
            return Err(BrokerStorageError::PostHaltEvent);
        }
        if envelope.sequence != expected {
            return Err(BrokerStorageError::Sequence {
                expected,
                actual: envelope.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(BrokerStorageError::SequenceExhausted)?;
        if envelope.source != EventSource::System || envelope.market_id != STREAM {
            return Err(BrokerStorageError::EnvelopeIdentity);
        }
        let command = decode_command(&envelope.payload)?;
        if envelope.event_time_ns != command.recorded_at_ns()
            || envelope.received_time_ns != command.recorded_at_ns()
        {
            return Err(BrokerStorageError::EnvelopeTimestamp);
        }
        if owner.apply(&command).is_err() {
            if !owner.is_halted() {
                return Err(BrokerStorageError::Replay);
            }
            halted = true;
        }
        last = Some(envelope.sequence);
        if checkpoint.is_some_and(|v| v.sequence == envelope.sequence) {
            if owner.snapshot().digest
                != checkpoint
                    .ok_or(BrokerStorageError::CheckpointMismatch)?
                    .state_digest
            {
                return Err(BrokerStorageError::CheckpointMismatch);
            }
            verified = true;
        }
    }
    if !verified {
        return Err(BrokerStorageError::CheckpointSequenceMissing);
    }
    Ok(BrokerRecovery {
        owner,
        last_sequence: last,
    })
}
/// Creates and syncs one checkpoint without replacement.
///
/// # Errors
///
/// Returns I/O errors, including an existing target.
pub fn write_checkpoint_create_new(
    path: impl AsRef<Path>,
    checkpoint: BrokerCheckpoint,
) -> Result<(), BrokerStorageError> {
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
pub fn read_checkpoint(path: impl AsRef<Path>) -> Result<BrokerCheckpoint, BrokerStorageError> {
    let bytes = fs::read(path)?;
    if bytes.len() != BYTES || bytes.get(..8) != Some(MAGIC) {
        return Err(BrokerStorageError::CheckpointLength);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| BrokerStorageError::CheckpointLength)?,
    );
    if version != VERSION {
        return Err(BrokerStorageError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(BrokerStorageError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..BODY]).as_bytes() != &bytes[BODY..] {
        return Err(BrokerStorageError::CheckpointChecksum);
    }
    Ok(BrokerCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| BrokerStorageError::CheckpointLength)?,
        ),
        state_digest: bytes[24..56]
            .try_into()
            .map_err(|_| BrokerStorageError::CheckpointLength)?,
    })
}
#[derive(Debug, Error)]
pub enum BrokerStorageError {
    #[error("credential broker error: {0}")]
    Broker(#[from] BrokerError),
    #[error("broker envelope error: {0:?}")]
    Schema(#[from] SchemaError),
    #[error("broker journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("broker segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("broker checkpoint I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("broker recovery mismatch")]
    RecoveryMismatch,
    #[error("broker sequence expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("broker sequence exhausted")]
    SequenceExhausted,
    #[error("broker envelope identity invalid")]
    EnvelopeIdentity,
    #[error("broker envelope timestamp invalid")]
    EnvelopeTimestamp,
    #[error("broker replay failed without halt")]
    Replay,
    #[error("broker post-halt event exists")]
    PostHaltEvent,
    #[error("broker checkpoint length invalid")]
    CheckpointLength,
    #[error("unsupported broker checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("broker checkpoint reserved bytes non-zero")]
    CheckpointReserved,
    #[error("broker checkpoint checksum invalid")]
    CheckpointChecksum,
    #[error("broker checkpoint mismatch")]
    CheckpointMismatch,
    #[error("broker checkpoint sequence missing")]
    CheckpointSequenceMissing,
    #[error("broker durable owner halted: {0}")]
    Halted(String),
}
