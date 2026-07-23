use crate::CanaryEligibilityRecord;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

const MAGIC: &[u8; 8] = b"POLYPGR1";
const VERSION: u16 = 1;
const HEADER_BYTES: usize = 24;
const CHECKSUM_BYTES: usize = 32;
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordBody {
    version: u16,
    record: CanaryEligibilityRecord,
}

/// Creates and device-syncs one canonical canary record without replacement.
///
/// # Errors
///
/// Returns serialization, size, I/O, existing-target, or digest failures.
pub fn write_canary_record_create_new(
    path: impl AsRef<Path>,
    record: &CanaryEligibilityRecord,
) -> Result<(), CanaryRecordFileError> {
    if !record.verify_digest() {
        return Err(CanaryRecordFileError::RecordDigest);
    }
    let body = serde_json::to_vec(&RecordBody {
        version: VERSION,
        record: record.clone(),
    })
    .map_err(|error| CanaryRecordFileError::Json(error.to_string()))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(CanaryRecordFileError::Length);
    }
    let body_len = u64::try_from(body.len()).map_err(|_| CanaryRecordFileError::Length)?;
    let mut bytes = Vec::with_capacity(HEADER_BYTES + body.len() + CHECKSUM_BYTES);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&[0; 6]);
    bytes.extend_from_slice(&body_len.to_le_bytes());
    bytes.extend_from_slice(&body);
    let checksum = blake3::hash(&bytes);
    bytes.extend_from_slice(checksum.as_bytes());
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Reads and fully verifies one canary-eligibility record.
///
/// # Errors
///
/// Rejects malformed, oversized, noncanonical, corrupt, or digest-invalid data.
pub fn read_canary_record(
    path: impl AsRef<Path>,
) -> Result<CanaryEligibilityRecord, CanaryRecordFileError> {
    let bytes = fs::read(path)?;
    if bytes.len() < HEADER_BYTES + CHECKSUM_BYTES || bytes.get(0..8) != Some(MAGIC) {
        return Err(CanaryRecordFileError::Length);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| CanaryRecordFileError::Length)?,
    );
    if version != VERSION {
        return Err(CanaryRecordFileError::Version(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(CanaryRecordFileError::Reserved);
    }
    let body_len = usize::try_from(u64::from_le_bytes(
        bytes[16..24]
            .try_into()
            .map_err(|_| CanaryRecordFileError::Length)?,
    ))
    .map_err(|_| CanaryRecordFileError::Length)?;
    if body_len > MAX_BODY_BYTES || bytes.len() != HEADER_BYTES + body_len + CHECKSUM_BYTES {
        return Err(CanaryRecordFileError::Length);
    }
    let checksum_at = HEADER_BYTES + body_len;
    if blake3::hash(&bytes[..checksum_at]).as_bytes() != &bytes[checksum_at..] {
        return Err(CanaryRecordFileError::Checksum);
    }
    let mut deserializer = serde_json::Deserializer::from_slice(&bytes[HEADER_BYTES..checksum_at]);
    let body = RecordBody::deserialize(&mut deserializer)
        .map_err(|error| CanaryRecordFileError::Json(error.to_string()))?;
    deserializer
        .end()
        .map_err(|error| CanaryRecordFileError::Json(error.to_string()))?;
    if body.version != VERSION {
        return Err(CanaryRecordFileError::Version(body.version));
    }
    let canonical = serde_json::to_vec(&body)
        .map_err(|error| CanaryRecordFileError::Json(error.to_string()))?;
    if canonical != bytes[HEADER_BYTES..checksum_at] {
        return Err(CanaryRecordFileError::NonCanonical);
    }
    if !body.record.verify_digest() {
        return Err(CanaryRecordFileError::RecordDigest);
    }
    Ok(body.record)
}

#[derive(Debug, Error)]
pub enum CanaryRecordFileError {
    #[error("canary record I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("canary record JSON is invalid: {0}")]
    Json(String),
    #[error("canary record length or magic is invalid")]
    Length,
    #[error("unsupported canary record version: {0}")]
    Version(u16),
    #[error("canary record reserved bytes are non-zero")]
    Reserved,
    #[error("canary record checksum is invalid")]
    Checksum,
    #[error("canary record body is not canonical")]
    NonCanonical,
    #[error("canary record internal digest is invalid")]
    RecordDigest,
}
