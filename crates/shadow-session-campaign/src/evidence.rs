use crate::OperatorEvidenceBundle;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

const MAGIC: &[u8; 8] = b"POLYSCE1";
const VERSION: u16 = 1;
const HEADER_BYTES: usize = 24;
const CHECKSUM_BYTES: usize = 32;
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct EvidenceBody {
    version: u16,
    bundle: OperatorEvidenceBundle,
}

/// Creates and device-syncs one canonical evidence file without replacement.
///
/// # Errors
///
/// Returns serialization, size, I/O, or existing-target failures.
pub fn write_evidence_bundle_create_new(
    path: impl AsRef<Path>,
    bundle: &OperatorEvidenceBundle,
) -> Result<(), EvidenceFileError> {
    if !bundle.verify_digest() {
        return Err(EvidenceFileError::BundleDigest);
    }
    let body = serde_json::to_vec(&EvidenceBody {
        version: VERSION,
        bundle: bundle.clone(),
    })
    .map_err(|error| EvidenceFileError::Json(error.to_string()))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(EvidenceFileError::Length);
    }
    let body_len = u64::try_from(body.len()).map_err(|_| EvidenceFileError::Length)?;
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

/// Reads and fully verifies one campaign evidence file.
///
/// # Errors
///
/// Rejects malformed, oversized, non-canonical, corrupt, or digest-invalid data.
pub fn read_evidence_bundle(
    path: impl AsRef<Path>,
) -> Result<OperatorEvidenceBundle, EvidenceFileError> {
    let bytes = fs::read(path)?;
    if bytes.len() < HEADER_BYTES + CHECKSUM_BYTES || bytes.get(0..8) != Some(MAGIC) {
        return Err(EvidenceFileError::Length);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| EvidenceFileError::Length)?,
    );
    if version != VERSION {
        return Err(EvidenceFileError::Version(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(EvidenceFileError::Reserved);
    }
    let body_len = usize::try_from(u64::from_le_bytes(
        bytes[16..24]
            .try_into()
            .map_err(|_| EvidenceFileError::Length)?,
    ))
    .map_err(|_| EvidenceFileError::Length)?;
    if body_len > MAX_BODY_BYTES || bytes.len() != HEADER_BYTES + body_len + CHECKSUM_BYTES {
        return Err(EvidenceFileError::Length);
    }
    let checksum_at = HEADER_BYTES + body_len;
    if blake3::hash(&bytes[..checksum_at]).as_bytes() != &bytes[checksum_at..] {
        return Err(EvidenceFileError::Checksum);
    }
    let mut deserializer = serde_json::Deserializer::from_slice(&bytes[HEADER_BYTES..checksum_at]);
    let body = EvidenceBody::deserialize(&mut deserializer)
        .map_err(|error| EvidenceFileError::Json(error.to_string()))?;
    deserializer
        .end()
        .map_err(|error| EvidenceFileError::Json(error.to_string()))?;
    if body.version != VERSION {
        return Err(EvidenceFileError::Version(body.version));
    }
    let canonical =
        serde_json::to_vec(&body).map_err(|error| EvidenceFileError::Json(error.to_string()))?;
    if canonical != bytes[HEADER_BYTES..checksum_at] {
        return Err(EvidenceFileError::NonCanonical);
    }
    if !body.bundle.verify_digest() {
        return Err(EvidenceFileError::BundleDigest);
    }
    Ok(body.bundle)
}

#[derive(Debug, Error)]
pub enum EvidenceFileError {
    #[error("campaign evidence I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("campaign evidence JSON is invalid: {0}")]
    Json(String),
    #[error("campaign evidence length or magic is invalid")]
    Length,
    #[error("unsupported campaign evidence version: {0}")]
    Version(u16),
    #[error("campaign evidence reserved bytes are non-zero")]
    Reserved,
    #[error("campaign evidence checksum is invalid")]
    Checksum,
    #[error("campaign evidence body is not canonical")]
    NonCanonical,
    #[error("campaign evidence internal bundle digest is invalid")]
    BundleDigest,
}
