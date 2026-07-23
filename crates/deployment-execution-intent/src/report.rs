use crate::ExecutionCertificationReport;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};
use thiserror::Error;

const MAGIC: &[u8; 8] = b"POLYEIR1";
const VERSION: u16 = 1;
const HEADER: usize = 24;
const CHECKSUM: usize = 32;
const MAX_BODY: usize = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    version: u16,
    report: ExecutionCertificationReport,
}

/// Creates and syncs one canonical report without replacement.
///
/// # Errors
///
/// Returns digest, serialization, size, I/O, or existing-target failures.
pub fn write_report_create_new(
    path: impl AsRef<Path>,
    report: &ExecutionCertificationReport,
) -> Result<(), ExecutionReportFileError> {
    if !report.verify_digest() {
        return Err(ExecutionReportFileError::Digest);
    }
    let body = serde_json::to_vec(&Body {
        version: VERSION,
        report: report.clone(),
    })
    .map_err(|e| ExecutionReportFileError::Json(e.to_string()))?;
    if body.len() > MAX_BODY {
        return Err(ExecutionReportFileError::Length);
    }
    let length = u64::try_from(body.len()).map_err(|_| ExecutionReportFileError::Length)?;
    let mut bytes = Vec::with_capacity(HEADER + body.len() + CHECKSUM);
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

/// Reads and fully verifies one canonical report.
///
/// # Errors
///
/// Rejects malformed, oversized, noncanonical, corrupt, or digest-invalid data.
pub fn read_report(
    path: impl AsRef<Path>,
) -> Result<ExecutionCertificationReport, ExecutionReportFileError> {
    let bytes = fs::read(path)?;
    if bytes.len() < HEADER + CHECKSUM || bytes.get(..8) != Some(MAGIC) {
        return Err(ExecutionReportFileError::Length);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| ExecutionReportFileError::Length)?,
    );
    if version != VERSION {
        return Err(ExecutionReportFileError::Version(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(ExecutionReportFileError::Reserved);
    }
    let length = usize::try_from(u64::from_le_bytes(
        bytes[16..24]
            .try_into()
            .map_err(|_| ExecutionReportFileError::Length)?,
    ))
    .map_err(|_| ExecutionReportFileError::Length)?;
    if length > MAX_BODY || bytes.len() != HEADER + length + CHECKSUM {
        return Err(ExecutionReportFileError::Length);
    }
    let end = HEADER + length;
    if blake3::hash(&bytes[..end]).as_bytes() != &bytes[end..] {
        return Err(ExecutionReportFileError::Checksum);
    }
    let mut de = serde_json::Deserializer::from_slice(&bytes[HEADER..end]);
    let body =
        Body::deserialize(&mut de).map_err(|e| ExecutionReportFileError::Json(e.to_string()))?;
    de.end()
        .map_err(|e| ExecutionReportFileError::Json(e.to_string()))?;
    if body.version != VERSION {
        return Err(ExecutionReportFileError::Version(body.version));
    }
    if serde_json::to_vec(&body).map_err(|e| ExecutionReportFileError::Json(e.to_string()))?
        != bytes[HEADER..end]
    {
        return Err(ExecutionReportFileError::NonCanonical);
    }
    if !body.report.verify_digest() {
        return Err(ExecutionReportFileError::Digest);
    }
    Ok(body.report)
}

#[derive(Debug, Error)]
pub enum ExecutionReportFileError {
    #[error("execution report I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("execution report JSON error: {0}")]
    Json(String),
    #[error("execution report length or magic is invalid")]
    Length,
    #[error("unsupported execution report version: {0}")]
    Version(u16),
    #[error("execution report reserved bytes are non-zero")]
    Reserved,
    #[error("execution report checksum is invalid")]
    Checksum,
    #[error("execution report body is not canonical")]
    NonCanonical,
    #[error("execution report digest is invalid")]
    Digest,
}
