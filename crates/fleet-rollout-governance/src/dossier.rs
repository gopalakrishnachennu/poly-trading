use crate::OperationalReadinessDossier;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

const MAGIC: &[u8; 8] = b"POLYFDR1";
const VERSION: u16 = 1;
const HEADER_BYTES: usize = 24;
const CHECKSUM_BYTES: usize = 32;
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DossierBody {
    version: u16,
    dossier: OperationalReadinessDossier,
}

/// Creates and device-syncs one canonical dossier without replacement.
///
/// # Errors
///
/// Returns serialization, size, I/O, existing-target, or digest failures.
pub fn write_dossier_create_new(
    path: impl AsRef<Path>,
    dossier: &OperationalReadinessDossier,
) -> Result<(), DossierFileError> {
    if !dossier.verify_digest() {
        return Err(DossierFileError::DossierDigest);
    }
    let body = serde_json::to_vec(&DossierBody {
        version: VERSION,
        dossier: dossier.clone(),
    })
    .map_err(|error| DossierFileError::Json(error.to_string()))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(DossierFileError::Length);
    }
    let body_len = u64::try_from(body.len()).map_err(|_| DossierFileError::Length)?;
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

/// Reads and fully verifies one readiness dossier.
///
/// # Errors
///
/// Rejects malformed, oversized, noncanonical, corrupt, or digest-invalid data.
pub fn read_dossier(
    path: impl AsRef<Path>,
) -> Result<OperationalReadinessDossier, DossierFileError> {
    let bytes = fs::read(path)?;
    if bytes.len() < HEADER_BYTES + CHECKSUM_BYTES || bytes.get(0..8) != Some(MAGIC) {
        return Err(DossierFileError::Length);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| DossierFileError::Length)?,
    );
    if version != VERSION {
        return Err(DossierFileError::Version(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(DossierFileError::Reserved);
    }
    let body_len = usize::try_from(u64::from_le_bytes(
        bytes[16..24]
            .try_into()
            .map_err(|_| DossierFileError::Length)?,
    ))
    .map_err(|_| DossierFileError::Length)?;
    if body_len > MAX_BODY_BYTES || bytes.len() != HEADER_BYTES + body_len + CHECKSUM_BYTES {
        return Err(DossierFileError::Length);
    }
    let checksum_at = HEADER_BYTES + body_len;
    if blake3::hash(&bytes[..checksum_at]).as_bytes() != &bytes[checksum_at..] {
        return Err(DossierFileError::Checksum);
    }
    let mut deserializer = serde_json::Deserializer::from_slice(&bytes[HEADER_BYTES..checksum_at]);
    let body = DossierBody::deserialize(&mut deserializer)
        .map_err(|error| DossierFileError::Json(error.to_string()))?;
    deserializer
        .end()
        .map_err(|error| DossierFileError::Json(error.to_string()))?;
    if body.version != VERSION {
        return Err(DossierFileError::Version(body.version));
    }
    let canonical =
        serde_json::to_vec(&body).map_err(|error| DossierFileError::Json(error.to_string()))?;
    if canonical != bytes[HEADER_BYTES..checksum_at] {
        return Err(DossierFileError::NonCanonical);
    }
    if !body.dossier.verify_digest() {
        return Err(DossierFileError::DossierDigest);
    }
    Ok(body.dossier)
}

#[derive(Debug, Error)]
pub enum DossierFileError {
    #[error("fleet dossier I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("fleet dossier JSON is invalid: {0}")]
    Json(String),
    #[error("fleet dossier length or magic is invalid")]
    Length,
    #[error("unsupported fleet dossier version: {0}")]
    Version(u16),
    #[error("fleet dossier reserved bytes are non-zero")]
    Reserved,
    #[error("fleet dossier checksum is invalid")]
    Checksum,
    #[error("fleet dossier body is not canonical")]
    NonCanonical,
    #[error("fleet dossier internal digest is invalid")]
    DossierDigest,
}
