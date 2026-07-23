use crate::ProductionReadinessRecord;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

const MAGIC: &[u8; 8] = b"POLYPCR1";
const VERSION: u16 = 1;
const HEADER_BYTES: usize = 24;
const CHECKSUM_BYTES: usize = 32;
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct RecordBody {
    version: u16,
    record: ProductionReadinessRecord,
}

/// Creates and device-syncs one canonical record without replacement.
///
/// # Errors
///
/// Returns serialization, size, I/O, existing-target or digest failures.
pub fn write_record_create_new(
    path: impl AsRef<Path>,
    record: &ProductionReadinessRecord,
) -> Result<(), ProductionReadinessRecordFileError> {
    if !record.verify_digest() {
        return Err(ProductionReadinessRecordFileError::RecordDigest);
    }
    let body = serde_json::to_vec(&RecordBody {
        version: VERSION,
        record: record.clone(),
    })
    .map_err(|error| ProductionReadinessRecordFileError::Json(error.to_string()))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(ProductionReadinessRecordFileError::Length);
    }
    let body_len =
        u64::try_from(body.len()).map_err(|_| ProductionReadinessRecordFileError::Length)?;
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

/// Reads and fully verifies one canonical readiness record.
///
/// # Errors
///
/// Rejects malformed, oversized, noncanonical, corrupt or digest-invalid data.
pub fn read_record(
    path: impl AsRef<Path>,
) -> Result<ProductionReadinessRecord, ProductionReadinessRecordFileError> {
    let bytes = fs::read(path)?;
    if bytes.len() < HEADER_BYTES + CHECKSUM_BYTES || bytes.get(0..8) != Some(MAGIC) {
        return Err(ProductionReadinessRecordFileError::Length);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| ProductionReadinessRecordFileError::Length)?,
    );
    if version != VERSION {
        return Err(ProductionReadinessRecordFileError::Version(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(ProductionReadinessRecordFileError::Reserved);
    }
    let body_len = usize::try_from(u64::from_le_bytes(
        bytes[16..24]
            .try_into()
            .map_err(|_| ProductionReadinessRecordFileError::Length)?,
    ))
    .map_err(|_| ProductionReadinessRecordFileError::Length)?;
    if body_len > MAX_BODY_BYTES || bytes.len() != HEADER_BYTES + body_len + CHECKSUM_BYTES {
        return Err(ProductionReadinessRecordFileError::Length);
    }
    let checksum_at = HEADER_BYTES + body_len;
    if blake3::hash(&bytes[..checksum_at]).as_bytes() != &bytes[checksum_at..] {
        return Err(ProductionReadinessRecordFileError::Checksum);
    }
    let mut deserializer = serde_json::Deserializer::from_slice(&bytes[HEADER_BYTES..checksum_at]);
    let body = RecordBody::deserialize(&mut deserializer)
        .map_err(|error| ProductionReadinessRecordFileError::Json(error.to_string()))?;
    deserializer
        .end()
        .map_err(|error| ProductionReadinessRecordFileError::Json(error.to_string()))?;
    if body.version != VERSION {
        return Err(ProductionReadinessRecordFileError::Version(body.version));
    }
    if serde_json::to_vec(&body)
        .map_err(|error| ProductionReadinessRecordFileError::Json(error.to_string()))?
        != bytes[HEADER_BYTES..checksum_at]
    {
        return Err(ProductionReadinessRecordFileError::NonCanonical);
    }
    if !body.record.verify_digest() {
        return Err(ProductionReadinessRecordFileError::RecordDigest);
    }
    Ok(body.record)
}

#[derive(Debug, Error)]
pub enum ProductionReadinessRecordFileError {
    #[error("production readiness record I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("production readiness record JSON is invalid: {0}")]
    Json(String),
    #[error("production readiness record length or magic is invalid")]
    Length,
    #[error("unsupported production readiness record version: {0}")]
    Version(u16),
    #[error("production readiness record reserved bytes are non-zero")]
    Reserved,
    #[error("production readiness record checksum is invalid")]
    Checksum,
    #[error("production readiness record body is not canonical")]
    NonCanonical,
    #[error("production readiness record internal digest is invalid")]
    RecordDigest,
}
