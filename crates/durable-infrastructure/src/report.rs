use crate::InfrastructureReport;
use serde::{Deserialize, Serialize};
use std::{fs, fs::OpenOptions, io::Write, path::Path};
use thiserror::Error;

const MAGIC: &[u8; 8] = b"POLYDIR1";
const VERSION: u16 = 1;
const HEADER_BYTES: usize = 24;
const CHECKSUM_BYTES: usize = 32;
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    version: u16,
    report: InfrastructureReport,
}

/// Creates and syncs one canonical report without replacement.
///
/// # Errors
///
/// Returns digest, serialization, size, I/O, or existing-target failures.
pub fn write_report_create_new(
    path: impl AsRef<Path>,
    report: &InfrastructureReport,
) -> Result<(), InfrastructureReportFileError> {
    if !report.verify_digest() {
        return Err(InfrastructureReportFileError::Digest);
    }
    let body = serde_json::to_vec(&Body {
        version: VERSION,
        report: report.clone(),
    })
    .map_err(|error| InfrastructureReportFileError::Json(error.to_string()))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(InfrastructureReportFileError::Length);
    }
    let length = u64::try_from(body.len()).map_err(|_| InfrastructureReportFileError::Length)?;
    let mut bytes = Vec::with_capacity(HEADER_BYTES + body.len() + CHECKSUM_BYTES);
    bytes.extend_from_slice(MAGIC);
    bytes.extend_from_slice(&VERSION.to_le_bytes());
    bytes.extend_from_slice(&[0; 6]);
    bytes.extend_from_slice(&length.to_le_bytes());
    bytes.extend_from_slice(&body);
    let checksum = blake3::hash(&bytes);
    bytes.extend_from_slice(checksum.as_bytes());
    let mut file = OpenOptions::new().create_new(true).write(true).open(path)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    Ok(())
}

/// Reads and verifies one canonical report.
///
/// # Errors
///
/// Rejects malformed, oversized, noncanonical, corrupt, or digest-invalid data.
pub fn read_report(
    path: impl AsRef<Path>,
) -> Result<InfrastructureReport, InfrastructureReportFileError> {
    let bytes = fs::read(path)?;
    if bytes.len() < HEADER_BYTES + CHECKSUM_BYTES || bytes.get(..8) != Some(MAGIC) {
        return Err(InfrastructureReportFileError::Length);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| InfrastructureReportFileError::Length)?,
    );
    if version != VERSION {
        return Err(InfrastructureReportFileError::Version(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(InfrastructureReportFileError::Reserved);
    }
    let length = usize::try_from(u64::from_le_bytes(
        bytes[16..24]
            .try_into()
            .map_err(|_| InfrastructureReportFileError::Length)?,
    ))
    .map_err(|_| InfrastructureReportFileError::Length)?;
    if length > MAX_BODY_BYTES || bytes.len() != HEADER_BYTES + length + CHECKSUM_BYTES {
        return Err(InfrastructureReportFileError::Length);
    }
    let end = HEADER_BYTES + length;
    if blake3::hash(&bytes[..end]).as_bytes() != &bytes[end..] {
        return Err(InfrastructureReportFileError::Checksum);
    }
    let mut decoder = serde_json::Deserializer::from_slice(&bytes[HEADER_BYTES..end]);
    let body = Body::deserialize(&mut decoder)
        .map_err(|error| InfrastructureReportFileError::Json(error.to_string()))?;
    decoder
        .end()
        .map_err(|error| InfrastructureReportFileError::Json(error.to_string()))?;
    if body.version != VERSION {
        return Err(InfrastructureReportFileError::Version(body.version));
    }
    if serde_json::to_vec(&body)
        .map_err(|error| InfrastructureReportFileError::Json(error.to_string()))?
        != bytes[HEADER_BYTES..end]
    {
        return Err(InfrastructureReportFileError::NonCanonical);
    }
    if !body.report.verify_digest() {
        return Err(InfrastructureReportFileError::Digest);
    }
    Ok(body.report)
}

#[derive(Debug, Error)]
pub enum InfrastructureReportFileError {
    #[error("infrastructure report I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("infrastructure report JSON error: {0}")]
    Json(String),
    #[error("infrastructure report length or magic invalid")]
    Length,
    #[error("unsupported infrastructure report version: {0}")]
    Version(u16),
    #[error("infrastructure report reserved bytes non-zero")]
    Reserved,
    #[error("infrastructure report checksum invalid")]
    Checksum,
    #[error("infrastructure report noncanonical")]
    NonCanonical,
    #[error("infrastructure report digest invalid")]
    Digest,
}
