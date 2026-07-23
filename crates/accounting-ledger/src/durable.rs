use crate::{
    decode_command, encode_command, AccountingLedger, ApplyOutcome, Error as AccountingError,
    LedgerCommand,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

const LEDGER_STREAM_ID: &str = "__accounting_ledger__";
const CHECKPOINT_MAGIC: &[u8; 8] = b"POLYALP1";
const CHECKPOINT_VERSION: u16 = 1;
const CHECKPOINT_BODY_BYTES: usize = 56;
const CHECKPOINT_BYTES: usize = 88;

/// Durable-prefix binding between a journal sequence and ledger state.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LedgerCheckpoint {
    pub sequence: u64,
    pub ledger_digest: [u8; 32],
}

#[derive(Debug)]
pub struct LedgerRecovery {
    pub ledger: AccountingLedger,
    pub last_sequence: Option<u64>,
}

/// Journal-first accounting owner. After an append, sync, or post-sync apply
/// failure, only restart recovery can determine durable truth.
#[derive(Debug)]
pub struct DurableLedger<J> {
    journal: J,
    ledger: AccountingLedger,
    last_sequence: Option<u64>,
    poisoned: Option<String>,
}

impl<J: EventJournal> DurableLedger<J> {
    /// Aligns an opened writer with strictly recovered state.
    ///
    /// # Errors
    ///
    /// Rejects any writer/recovery sequence mismatch.
    pub fn new(journal: J, recovery: LedgerRecovery) -> Result<Self, PersistenceError> {
        if journal.last_event_sequence() != recovery.last_sequence {
            return Err(PersistenceError::RecoveryMismatch);
        }
        Ok(Self {
            journal,
            ledger: recovery.ledger,
            last_sequence: recovery.last_sequence,
            poisoned: None,
        })
    }

    /// Validates, appends, device-syncs, and only then mutates live state.
    ///
    /// # Errors
    ///
    /// Rejected business commands are not journaled. Failures after append
    /// poison the live owner and require restart recovery.
    pub fn apply(&mut self, command: &LedgerCommand) -> Result<ApplyOutcome, PersistenceError> {
        if let Some(reason) = &self.poisoned {
            return Err(PersistenceError::Halted(reason.clone()));
        }
        let mut preflight = self.ledger.clone();
        let preflight_result = preflight.apply(command);
        let conflict = matches!(preflight_result, Err(AccountingError::IdempotencyConflict));
        let expected = match preflight_result {
            Ok(outcome) => outcome,
            Err(AccountingError::IdempotencyConflict) => ApplyOutcome::Duplicate,
            Err(error) => return Err(error.into()),
        };
        let sequence = self.last_sequence.map_or(Ok(0), |value| {
            value
                .checked_add(1)
                .ok_or(PersistenceError::SequenceExhausted)
        })?;
        let timestamp = command.recorded_at_ns();
        let envelope = EventEnvelope::new(
            EventSource::System,
            sequence,
            timestamp,
            timestamp,
            LEDGER_STREAM_ID.to_owned(),
            encode_command(command)?,
        )?;
        if let Err(error) = self.journal.append_event(&envelope) {
            return self.poison(PersistenceError::Journal(error));
        }
        self.last_sequence = Some(sequence);
        if let Err(error) = self.journal.sync_events() {
            return self.poison(PersistenceError::Journal(error));
        }
        self.ledger = preflight;
        if conflict {
            return Err(PersistenceError::Accounting(
                AccountingError::IdempotencyConflict,
            ));
        }
        Ok(expected)
    }

    /// Synchronizes the current durable prefix.
    ///
    /// # Errors
    ///
    /// A synchronization failure poisons the live owner.
    pub fn sync(&mut self) -> Result<(), PersistenceError> {
        if let Some(reason) = &self.poisoned {
            return Err(PersistenceError::Halted(reason.clone()));
        }
        self.journal
            .sync_events()
            .map_err(PersistenceError::Journal)
            .or_else(|error| self.poison(error))
    }

    #[must_use]
    pub const fn ledger(&self) -> &AccountingLedger {
        &self.ledger
    }

    #[must_use]
    pub const fn last_sequence(&self) -> Option<u64> {
        self.last_sequence
    }

    #[must_use]
    pub const fn journal(&self) -> &J {
        &self.journal
    }

    fn poison<T>(&mut self, error: PersistenceError) -> Result<T, PersistenceError> {
        self.poisoned = Some(error.to_string());
        Err(error)
    }
}

/// Replays a complete segmented ledger journal and optionally validates a
/// prefix checkpoint.
///
/// # Errors
///
/// Rejects segment corruption, gaps, wrong envelopes, invalid commands,
/// failed accounting transitions, and checkpoint inconsistencies.
pub fn recover_segmented(
    directory: impl AsRef<Path>,
    checkpoint: Option<LedgerCheckpoint>,
) -> Result<LedgerRecovery, PersistenceError> {
    let mut reader = SegmentedJournalReader::open(directory)?;
    let mut ledger = AccountingLedger::default();
    let mut expected_sequence = 0_u64;
    let mut last_sequence = None;
    let mut checkpoint_verified = checkpoint.is_none();
    while let Some(envelope) = reader.next_event()? {
        if envelope.sequence != expected_sequence {
            return Err(PersistenceError::Sequence {
                expected: expected_sequence,
                actual: envelope.sequence,
            });
        }
        expected_sequence = expected_sequence
            .checked_add(1)
            .ok_or(PersistenceError::SequenceExhausted)?;
        let command = validate_envelope(&envelope)?;
        ledger.apply(&command)?;
        last_sequence = Some(envelope.sequence);
        if checkpoint.is_some_and(|value| value.sequence == envelope.sequence) {
            let expected_digest = checkpoint
                .map(|value| value.ledger_digest)
                .ok_or(PersistenceError::CheckpointMismatch)?;
            if ledger.snapshot().digest != expected_digest {
                return Err(PersistenceError::CheckpointMismatch);
            }
            checkpoint_verified = true;
        }
    }
    if !checkpoint_verified {
        return Err(PersistenceError::CheckpointSequenceMissing);
    }
    Ok(LedgerRecovery {
        ledger,
        last_sequence,
    })
}

fn validate_envelope(envelope: &EventEnvelope) -> Result<LedgerCommand, PersistenceError> {
    if envelope.source != EventSource::System || envelope.market_id != LEDGER_STREAM_ID {
        return Err(PersistenceError::EnvelopeIdentity);
    }
    let command = decode_command(&envelope.payload)?;
    if envelope.event_time_ns != command.recorded_at_ns()
        || envelope.received_time_ns != command.recorded_at_ns()
    {
        return Err(PersistenceError::EnvelopeTimestamp);
    }
    Ok(command)
}

/// Creates and device-syncs a new checkpoint without replacing an existing
/// target.
///
/// # Errors
///
/// Returns I/O errors, including an already-existing target.
pub fn write_checkpoint_create_new(
    path: impl AsRef<Path>,
    checkpoint: LedgerCheckpoint,
) -> Result<(), PersistenceError> {
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
/// Rejects wrong length, magic, version, reserved bytes, or checksum.
pub fn read_checkpoint(path: impl AsRef<Path>) -> Result<LedgerCheckpoint, PersistenceError> {
    decode_checkpoint(&fs::read(path)?)
}

fn encode_checkpoint(checkpoint: LedgerCheckpoint) -> [u8; CHECKPOINT_BYTES] {
    let mut bytes = [0_u8; CHECKPOINT_BYTES];
    bytes[0..8].copy_from_slice(CHECKPOINT_MAGIC);
    bytes[8..10].copy_from_slice(&CHECKPOINT_VERSION.to_le_bytes());
    bytes[16..24].copy_from_slice(&checkpoint.sequence.to_le_bytes());
    bytes[24..56].copy_from_slice(&checkpoint.ledger_digest);
    let checksum = blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]);
    bytes[CHECKPOINT_BODY_BYTES..].copy_from_slice(checksum.as_bytes());
    bytes
}

fn decode_checkpoint(bytes: &[u8]) -> Result<LedgerCheckpoint, PersistenceError> {
    if bytes.len() != CHECKPOINT_BYTES {
        return Err(PersistenceError::CheckpointLength);
    }
    if bytes.get(0..8) != Some(CHECKPOINT_MAGIC) {
        return Err(PersistenceError::CheckpointMagic);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| PersistenceError::CheckpointLength)?,
    );
    if version != CHECKPOINT_VERSION {
        return Err(PersistenceError::CheckpointVersion(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(PersistenceError::CheckpointReserved);
    }
    if blake3::hash(&bytes[..CHECKPOINT_BODY_BYTES]).as_bytes() != &bytes[CHECKPOINT_BODY_BYTES..] {
        return Err(PersistenceError::CheckpointChecksum);
    }
    Ok(LedgerCheckpoint {
        sequence: u64::from_le_bytes(
            bytes[16..24]
                .try_into()
                .map_err(|_| PersistenceError::CheckpointLength)?,
        ),
        ledger_digest: bytes[24..56]
            .try_into()
            .map_err(|_| PersistenceError::CheckpointLength)?,
    })
}

#[derive(Debug, Error)]
pub enum PersistenceError {
    #[error("accounting error: {0}")]
    Accounting(#[from] AccountingError),
    #[error("ledger envelope error: {0:?}")]
    Schema(#[from] SchemaError),
    #[error("ledger journal error: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("ledger segment error: {0}")]
    Segment(#[from] SegmentError),
    #[error("ledger I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("ledger journal sequence mismatch: expected {expected}, got {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("ledger journal sequence is exhausted")]
    SequenceExhausted,
    #[error("ledger envelope source or identity is invalid")]
    EnvelopeIdentity,
    #[error("ledger envelope timestamp does not match its command")]
    EnvelopeTimestamp,
    #[error("journal writer and recovered ledger disagree")]
    RecoveryMismatch,
    #[error("ledger checkpoint digest does not match its prefix")]
    CheckpointMismatch,
    #[error("ledger checkpoint sequence is absent from the journal")]
    CheckpointSequenceMissing,
    #[error("ledger checkpoint length is invalid")]
    CheckpointLength,
    #[error("ledger checkpoint magic is invalid")]
    CheckpointMagic,
    #[error("unsupported ledger checkpoint version: {0}")]
    CheckpointVersion(u16),
    #[error("ledger checkpoint reserved bytes are non-zero")]
    CheckpointReserved,
    #[error("ledger checkpoint checksum mismatch")]
    CheckpointChecksum,
    #[error("durable ledger is halted: {0}")]
    Halted(String),
}
