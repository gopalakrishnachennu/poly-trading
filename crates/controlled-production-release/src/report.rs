use crate::ControlledReleaseReport;
use serde::{Deserialize, Serialize};
use std::{fs, fs::OpenOptions, io::Write, path::Path};
use thiserror::Error;

const MAGIC: &[u8; 8] = b"POLYCRR1";
const VERSION: u16 = 1;
const HEADER: usize = 24;
const CHECKSUM: usize = 32;
const MAX: usize = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    version: u16,
    report: ControlledReleaseReport,
}

/// Writes a canonical create-new release report.
/// # Errors
/// Rejects invalid, oversized, existing or I/O-failed outputs.
pub fn write_report_create_new(
    path: impl AsRef<Path>,
    report: &ControlledReleaseReport,
) -> Result<(), ReleaseReportFileError> {
    if !report.verify_digest() {
        return Err(ReleaseReportFileError::Digest);
    }
    let body = serde_json::to_vec(&Body {
        version: VERSION,
        report: report.clone(),
    })
    .map_err(|e| ReleaseReportFileError::Json(e.to_string()))?;
    if body.len() > MAX {
        return Err(ReleaseReportFileError::Length);
    }
    let length = u64::try_from(body.len()).map_err(|_| ReleaseReportFileError::Length)?;
    let mut bytes = Vec::new();
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

/// Reads and validates a canonical release report.
/// # Errors
/// Rejects malformed, corrupt, noncanonical or invalid data.
pub fn read_report(
    path: impl AsRef<Path>,
) -> Result<ControlledReleaseReport, ReleaseReportFileError> {
    let bytes = fs::read(path)?;
    if bytes.len() < HEADER + CHECKSUM || bytes.get(..8) != Some(MAGIC) {
        return Err(ReleaseReportFileError::Length);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| ReleaseReportFileError::Length)?,
    );
    if version != VERSION {
        return Err(ReleaseReportFileError::Version(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(ReleaseReportFileError::Reserved);
    }
    let length = usize::try_from(u64::from_le_bytes(
        bytes[16..24]
            .try_into()
            .map_err(|_| ReleaseReportFileError::Length)?,
    ))
    .map_err(|_| ReleaseReportFileError::Length)?;
    if length > MAX || bytes.len() != HEADER + length + CHECKSUM {
        return Err(ReleaseReportFileError::Length);
    }
    let end = HEADER + length;
    if blake3::hash(&bytes[..end]).as_bytes() != &bytes[end..] {
        return Err(ReleaseReportFileError::Checksum);
    }
    let mut deserializer = serde_json::Deserializer::from_slice(&bytes[HEADER..end]);
    let body = Body::deserialize(&mut deserializer)
        .map_err(|e| ReleaseReportFileError::Json(e.to_string()))?;
    deserializer
        .end()
        .map_err(|e| ReleaseReportFileError::Json(e.to_string()))?;
    if body.version != VERSION {
        return Err(ReleaseReportFileError::Version(body.version));
    }
    if serde_json::to_vec(&body).map_err(|e| ReleaseReportFileError::Json(e.to_string()))?
        != bytes[HEADER..end]
    {
        return Err(ReleaseReportFileError::NonCanonical);
    }
    if !body.report.verify_digest() {
        return Err(ReleaseReportFileError::Digest);
    }
    Ok(body.report)
}

#[derive(Debug, Error)]
pub enum ReleaseReportFileError {
    #[error("report I/O {0}")]
    Io(#[from] std::io::Error),
    #[error("report JSON {0}")]
    Json(String),
    #[error("report length")]
    Length,
    #[error("report version {0}")]
    Version(u16),
    #[error("report reserved")]
    Reserved,
    #[error("report checksum")]
    Checksum,
    #[error("report noncanonical")]
    NonCanonical,
    #[error("report digest")]
    Digest,
}
