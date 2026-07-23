use crate::ExecutorSessionDossier;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};
use thiserror::Error;

const MAGIC: &[u8; 8] = b"POLYESD2";
const VERSION: u16 = 2;
const HEADER: usize = 24;
const CHECKSUM: usize = 32;
const MAX_BODY: usize = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    version: u16,
    dossier: ExecutorSessionDossier,
}

/// Creates and syncs one canonical dossier without replacement.
///
/// # Errors
///
/// Returns digest, serialization, size, I/O, or existing-target failures.
pub fn write_dossier_create_new(
    path: impl AsRef<Path>,
    dossier: &ExecutorSessionDossier,
) -> Result<(), SessionDossierFileError> {
    if !dossier.verify_digest() {
        return Err(SessionDossierFileError::Digest);
    }
    let body = serde_json::to_vec(&Body {
        version: VERSION,
        dossier: dossier.clone(),
    })
    .map_err(|e| SessionDossierFileError::Json(e.to_string()))?;
    if body.len() > MAX_BODY {
        return Err(SessionDossierFileError::Length);
    }
    let length = u64::try_from(body.len()).map_err(|_| SessionDossierFileError::Length)?;
    let mut bytes = Vec::with_capacity(HEADER + body.len() + CHECKSUM);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&[0; 6]);
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(&body);
    let sum = blake3::hash(&bytes);
    bytes.extend_from_slice(sum.as_bytes());
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Reads and verifies one canonical dossier.
///
/// # Errors
///
/// Rejects malformed, oversized, noncanonical, corrupt, or digest-invalid data.
pub fn read_dossier(
    path: impl AsRef<Path>,
) -> Result<ExecutorSessionDossier, SessionDossierFileError> {
    let bytes = fs::read(path)?;
    if bytes.len() < HEADER + CHECKSUM || bytes.get(..8) != Some(MAGIC) {
        return Err(SessionDossierFileError::Length);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| SessionDossierFileError::Length)?,
    );
    if version != VERSION {
        return Err(SessionDossierFileError::Version(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(SessionDossierFileError::Reserved);
    }
    let length = usize::try_from(u64::from_le_bytes(
        bytes[16..24]
            .try_into()
            .map_err(|_| SessionDossierFileError::Length)?,
    ))
    .map_err(|_| SessionDossierFileError::Length)?;
    if length > MAX_BODY || bytes.len() != HEADER + length + CHECKSUM {
        return Err(SessionDossierFileError::Length);
    }
    let end = HEADER + length;
    if blake3::hash(&bytes[..end]).as_bytes() != &bytes[end..] {
        return Err(SessionDossierFileError::Checksum);
    }
    let mut de = serde_json::Deserializer::from_slice(&bytes[HEADER..end]);
    let body =
        Body::deserialize(&mut de).map_err(|e| SessionDossierFileError::Json(e.to_string()))?;
    de.end()
        .map_err(|e| SessionDossierFileError::Json(e.to_string()))?;
    if body.version != VERSION {
        return Err(SessionDossierFileError::Version(body.version));
    }
    if serde_json::to_vec(&body).map_err(|e| SessionDossierFileError::Json(e.to_string()))?
        != bytes[HEADER..end]
    {
        return Err(SessionDossierFileError::NonCanonical);
    }
    if !body.dossier.verify_digest() {
        return Err(SessionDossierFileError::Digest);
    }
    Ok(body.dossier)
}

#[derive(Debug, Error)]
pub enum SessionDossierFileError {
    #[error("executor-session dossier I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("executor-session dossier JSON error: {0}")]
    Json(String),
    #[error("executor-session dossier length or magic is invalid")]
    Length,
    #[error("unsupported executor-session dossier version: {0}")]
    Version(u16),
    #[error("executor-session dossier reserved bytes are non-zero")]
    Reserved,
    #[error("executor-session dossier checksum is invalid")]
    Checksum,
    #[error("executor-session dossier is not canonical")]
    NonCanonical,
    #[error("executor-session dossier digest is invalid")]
    Digest,
}
