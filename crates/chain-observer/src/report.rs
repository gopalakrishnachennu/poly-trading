use crate::ChainReport;
use serde::{Deserialize, Serialize};
use std::{fs, fs::OpenOptions, io::Write, path::Path};
use thiserror::Error;

const MAGIC: &[u8; 8] = b"POLYCOR1";
const VERSION: u16 = 1;
const HEADER_BYTES: usize = 24;
const CHECKSUM_BYTES: usize = 32;
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    version: u16,
    report: ChainReport,
}

/// Creates and syncs one canonical report without replacement.
/// # Errors
/// Rejects invalid reports, oversize data, serialization and I/O failures.
pub fn write_report_create_new(
    path: impl AsRef<Path>,
    report: &ChainReport,
) -> Result<(), ChainReportFileError> {
    if !report.verify_digest() {
        return Err(ChainReportFileError::Digest);
    }
    let body = serde_json::to_vec(&Body {
        version: VERSION,
        report: report.clone(),
    })
    .map_err(|e| ChainReportFileError::Json(e.to_string()))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(ChainReportFileError::Length);
    }
    let length = u64::try_from(body.len()).map_err(|_| ChainReportFileError::Length)?;
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
/// # Errors
/// Rejects malformed, oversized, noncanonical, corrupt or digest-invalid data.
pub fn read_report(path: impl AsRef<Path>) -> Result<ChainReport, ChainReportFileError> {
    let bytes = fs::read(path)?;
    if bytes.len() < HEADER_BYTES + CHECKSUM_BYTES || bytes.get(..8) != Some(MAGIC) {
        return Err(ChainReportFileError::Length);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| ChainReportFileError::Length)?,
    );
    if version != VERSION {
        return Err(ChainReportFileError::Version(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(ChainReportFileError::Reserved);
    }
    let length = usize::try_from(u64::from_le_bytes(
        bytes[16..24]
            .try_into()
            .map_err(|_| ChainReportFileError::Length)?,
    ))
    .map_err(|_| ChainReportFileError::Length)?;
    if length > MAX_BODY_BYTES || bytes.len() != HEADER_BYTES + length + CHECKSUM_BYTES {
        return Err(ChainReportFileError::Length);
    }
    let end = HEADER_BYTES + length;
    if blake3::hash(&bytes[..end]).as_bytes() != &bytes[end..] {
        return Err(ChainReportFileError::Checksum);
    }
    let mut de = serde_json::Deserializer::from_slice(&bytes[HEADER_BYTES..end]);
    let body = Body::deserialize(&mut de).map_err(|e| ChainReportFileError::Json(e.to_string()))?;
    de.end()
        .map_err(|e| ChainReportFileError::Json(e.to_string()))?;
    if body.version != VERSION {
        return Err(ChainReportFileError::Version(body.version));
    }
    if serde_json::to_vec(&body).map_err(|e| ChainReportFileError::Json(e.to_string()))?
        != bytes[HEADER_BYTES..end]
    {
        return Err(ChainReportFileError::NonCanonical);
    }
    if !body.report.verify_digest() {
        return Err(ChainReportFileError::Digest);
    }
    Ok(body.report)
}

#[derive(Debug, Error)]
pub enum ChainReportFileError {
    #[error("chain report I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("chain report JSON error: {0}")]
    Json(String),
    #[error("chain report length or magic invalid")]
    Length,
    #[error("unsupported chain report version: {0}")]
    Version(u16),
    #[error("chain report reserved bytes non-zero")]
    Reserved,
    #[error("chain report checksum invalid")]
    Checksum,
    #[error("chain report noncanonical")]
    NonCanonical,
    #[error("chain report digest invalid")]
    Digest,
}
