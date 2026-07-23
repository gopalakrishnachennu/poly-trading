use crate::TransportAdapterCertificate;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};
use thiserror::Error;

const MAGIC: &[u8; 8] = b"POLYTAC1";
const VERSION: u16 = 1;
const HEADER: usize = 24;
const CHECKSUM: usize = 32;
const MAX_BODY: usize = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    version: u16,
    certificate: TransportAdapterCertificate,
}

/// Creates and syncs one certificate without replacement.
///
/// # Errors
///
/// Returns digest, serialization, size, I/O, or existing-target failures.
pub fn write_certificate_create_new(
    path: impl AsRef<Path>,
    certificate: &TransportAdapterCertificate,
) -> Result<(), TransportCertificateFileError> {
    if !certificate.verify_digest() {
        return Err(TransportCertificateFileError::Digest);
    }
    let body = serde_json::to_vec(&Body {
        version: VERSION,
        certificate: certificate.clone(),
    })
    .map_err(|e| TransportCertificateFileError::Json(e.to_string()))?;
    if body.len() > MAX_BODY {
        return Err(TransportCertificateFileError::Length);
    }
    let length = u64::try_from(body.len()).map_err(|_| TransportCertificateFileError::Length)?;
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

/// Reads and verifies one canonical certificate.
///
/// # Errors
///
/// Rejects malformed, oversized, noncanonical, corrupt, or digest-invalid data.
pub fn read_certificate(
    path: impl AsRef<Path>,
) -> Result<TransportAdapterCertificate, TransportCertificateFileError> {
    let bytes = fs::read(path)?;
    if bytes.len() < HEADER + CHECKSUM || bytes.get(..8) != Some(MAGIC) {
        return Err(TransportCertificateFileError::Length);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| TransportCertificateFileError::Length)?,
    );
    if version != VERSION {
        return Err(TransportCertificateFileError::Version(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(TransportCertificateFileError::Reserved);
    }
    let length = usize::try_from(u64::from_le_bytes(
        bytes[16..24]
            .try_into()
            .map_err(|_| TransportCertificateFileError::Length)?,
    ))
    .map_err(|_| TransportCertificateFileError::Length)?;
    if length > MAX_BODY || bytes.len() != HEADER + length + CHECKSUM {
        return Err(TransportCertificateFileError::Length);
    }
    let end = HEADER + length;
    if blake3::hash(&bytes[..end]).as_bytes() != &bytes[end..] {
        return Err(TransportCertificateFileError::Checksum);
    }
    let mut de = serde_json::Deserializer::from_slice(&bytes[HEADER..end]);
    let body = Body::deserialize(&mut de)
        .map_err(|e| TransportCertificateFileError::Json(e.to_string()))?;
    de.end()
        .map_err(|e| TransportCertificateFileError::Json(e.to_string()))?;
    if body.version != VERSION {
        return Err(TransportCertificateFileError::Version(body.version));
    }
    if serde_json::to_vec(&body).map_err(|e| TransportCertificateFileError::Json(e.to_string()))?
        != bytes[HEADER..end]
    {
        return Err(TransportCertificateFileError::NonCanonical);
    }
    if !body.certificate.verify_digest() {
        return Err(TransportCertificateFileError::Digest);
    }
    Ok(body.certificate)
}

#[derive(Debug, Error)]
pub enum TransportCertificateFileError {
    #[error("transport certificate I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("transport certificate JSON error: {0}")]
    Json(String),
    #[error("transport certificate length or magic is invalid")]
    Length,
    #[error("unsupported transport certificate version: {0}")]
    Version(u16),
    #[error("transport certificate reserved bytes are non-zero")]
    Reserved,
    #[error("transport certificate checksum is invalid")]
    Checksum,
    #[error("transport certificate is not canonical")]
    NonCanonical,
    #[error("transport certificate digest is invalid")]
    Digest,
}
