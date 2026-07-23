use crate::{
    decode_command, encode_command, CanaryCommand, CanaryOutcome, CanaryPolicy, Error as CoreError,
    MicroCapitalCanaryController,
};
use event_schema::{EventEnvelope, EventSource, SchemaError};
use market_recorder::{EventJournal, JournalBackendError, SegmentError, SegmentedJournalReader};
use std::{fs, fs::OpenOptions, io::Write, path::Path};
use thiserror::Error;
const STREAM: &str = "__micro_canary__";
const MAGIC: &[u8; 8] = b"POLYMCC1";
const VERSION: u16 = 1;
const BODY: usize = 56;
const SIZE: usize = 88;
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CanaryCheckpoint {
    pub sequence: u64,
    pub state_digest: [u8; 32],
}
#[derive(Debug)]
pub struct CanaryRecovery {
    pub owner: MicroCapitalCanaryController,
    pub last_sequence: Option<u64>,
}
#[derive(Debug)]
pub struct DurableCanaryController<J> {
    journal: J,
    owner: MicroCapitalCanaryController,
    last: Option<u64>,
    poisoned: Option<String>,
}
impl<J: EventJournal> DurableCanaryController<J> {
    /// Aligns recovered state.
    /// # Errors
    /// Rejects sequence disagreement.
    pub fn new(j: J, r: CanaryRecovery) -> Result<Self, CanaryStorageError> {
        if j.last_event_sequence() != r.last_sequence {
            return Err(CanaryStorageError::RecoveryMismatch);
        }
        Ok(Self {
            journal: j,
            owner: r.owner,
            last: r.last_sequence,
            poisoned: None,
        })
    }
    /// Journals before transition.
    /// # Errors
    /// Returns storage or core failures.
    pub fn apply(&mut self, c: &CanaryCommand) -> Result<CanaryOutcome, CanaryStorageError> {
        if let Some(r) = &self.poisoned {
            return Err(CanaryStorageError::Halted(r.clone()));
        }
        let payload = encode_command(c)?;
        let mut next = self.owner.clone();
        let result = next.apply(c);
        let seq = self.last.map_or(Ok(0), |v| {
            v.checked_add(1)
                .ok_or(CanaryStorageError::SequenceExhausted)
        })?;
        let at = c.recorded_at_ns();
        let e = EventEnvelope::new(EventSource::System, seq, at, at, STREAM.to_owned(), payload)?;
        if let Err(x) = self.journal.append_event(&e) {
            return self.poison(CanaryStorageError::Journal(x));
        }
        self.last = Some(seq);
        if let Err(x) = self.journal.sync_events() {
            return self.poison(CanaryStorageError::Journal(x));
        }
        self.owner = next;
        result.map_err(CanaryStorageError::Core)
    }
    #[must_use]
    pub const fn owner(&self) -> &MicroCapitalCanaryController {
        &self.owner
    }
    fn poison<T>(&mut self, e: CanaryStorageError) -> Result<T, CanaryStorageError> {
        self.poisoned = Some(e.to_string());
        Err(e)
    }
}
/// Replays a segmented journal.
/// # Errors
/// Rejects corruption, gaps, invalid envelopes, post-halt and checkpoint mismatch.
pub fn recover_segmented(
    dir: impl AsRef<Path>,
    p: CanaryPolicy,
    cp: Option<CanaryCheckpoint>,
) -> Result<CanaryRecovery, CanaryStorageError> {
    let mut reader = SegmentedJournalReader::open(dir)?;
    let mut owner = MicroCapitalCanaryController::new(p)?;
    let mut expected = 0;
    let mut last = None;
    let mut verified = cp.is_none();
    let mut halted = false;
    while let Some(e) = reader.next_event()? {
        if halted {
            return Err(CanaryStorageError::PostHaltEvent);
        }
        if e.sequence != expected {
            return Err(CanaryStorageError::Sequence {
                expected,
                actual: e.sequence,
            });
        }
        expected = expected
            .checked_add(1)
            .ok_or(CanaryStorageError::SequenceExhausted)?;
        if e.source != EventSource::System || e.market_id != STREAM {
            return Err(CanaryStorageError::EnvelopeIdentity);
        }
        let c = decode_command(&e.payload)?;
        if e.event_time_ns != c.recorded_at_ns() || e.received_time_ns != c.recorded_at_ns() {
            return Err(CanaryStorageError::EnvelopeTimestamp);
        }
        if owner.apply(&c).is_err() {
            if !owner.is_halted() {
                return Err(CanaryStorageError::Replay);
            }
            halted = true;
        }
        last = Some(e.sequence);
        if cp.is_some_and(|v| v.sequence == e.sequence) {
            if owner.snapshot().digest
                != cp
                    .ok_or(CanaryStorageError::CheckpointMismatch)?
                    .state_digest
            {
                return Err(CanaryStorageError::CheckpointMismatch);
            }
            verified = true;
        }
    }
    if !verified {
        return Err(CanaryStorageError::CheckpointSequenceMissing);
    }
    Ok(CanaryRecovery {
        owner,
        last_sequence: last,
    })
}
/// Creates a checkpoint.
/// # Errors
/// Returns I/O including existing target.
pub fn write_checkpoint_create_new(
    path: impl AsRef<Path>,
    c: CanaryCheckpoint,
) -> Result<(), CanaryStorageError> {
    let mut b = [0_u8; SIZE];
    b[..8].copy_from_slice(MAGIC);
    b[8..10].copy_from_slice(&VERSION.to_le_bytes());
    b[16..24].copy_from_slice(&c.sequence.to_le_bytes());
    b[24..56].copy_from_slice(&c.state_digest);
    let s = blake3::hash(&b[..BODY]);
    b[BODY..].copy_from_slice(s.as_bytes());
    let mut f = OpenOptions::new().create_new(true).write(true).open(path)?;
    f.write_all(&b)?;
    f.sync_all()?;
    Ok(())
}
/// Reads a checkpoint.
/// # Errors
/// Rejects malformed or corrupt data.
pub fn read_checkpoint(path: impl AsRef<Path>) -> Result<CanaryCheckpoint, CanaryStorageError> {
    let b = fs::read(path)?;
    if b.len() != SIZE || b.get(..8) != Some(MAGIC) {
        return Err(CanaryStorageError::CheckpointLength);
    }
    let v = u16::from_le_bytes(
        b[8..10]
            .try_into()
            .map_err(|_| CanaryStorageError::CheckpointLength)?,
    );
    if v != VERSION {
        return Err(CanaryStorageError::CheckpointVersion(v));
    }
    if b[10..16] != [0; 6] {
        return Err(CanaryStorageError::CheckpointReserved);
    }
    if blake3::hash(&b[..BODY]).as_bytes() != &b[BODY..] {
        return Err(CanaryStorageError::CheckpointChecksum);
    }
    Ok(CanaryCheckpoint {
        sequence: u64::from_le_bytes(
            b[16..24]
                .try_into()
                .map_err(|_| CanaryStorageError::CheckpointLength)?,
        ),
        state_digest: b[24..56]
            .try_into()
            .map_err(|_| CanaryStorageError::CheckpointLength)?,
    })
}
#[derive(Debug, Error)]
pub enum CanaryStorageError {
    #[error("canary core: {0}")]
    Core(#[from] CoreError),
    #[error("canary schema: {0}")]
    Schema(#[from] SchemaError),
    #[error("canary journal: {0}")]
    Journal(#[from] JournalBackendError),
    #[error("canary segment: {0}")]
    Segment(#[from] SegmentError),
    #[error("canary I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("recovery mismatch")]
    RecoveryMismatch,
    #[error("sequence exhausted")]
    SequenceExhausted,
    #[error("sequence expected {expected} actual {actual}")]
    Sequence { expected: u64, actual: u64 },
    #[error("envelope identity")]
    EnvelopeIdentity,
    #[error("envelope timestamp")]
    EnvelopeTimestamp,
    #[error("replay")]
    Replay,
    #[error("post halt")]
    PostHaltEvent,
    #[error("checkpoint length")]
    CheckpointLength,
    #[error("checkpoint version {0}")]
    CheckpointVersion(u16),
    #[error("checkpoint reserved")]
    CheckpointReserved,
    #[error("checkpoint checksum")]
    CheckpointChecksum,
    #[error("checkpoint mismatch")]
    CheckpointMismatch,
    #[error("checkpoint sequence missing")]
    CheckpointSequenceMissing,
    #[error("durable halted: {0}")]
    Halted(String),
}
