use crate::AdapterCertificationReport;
use serde::{Deserialize, Serialize};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;
use thiserror::Error;

const MAGIC: &[u8; 8] = b"POLYDAR2";
const VERSION: u16 = 2;
const HEADER_BYTES: usize = 24;
const CHECKSUM_BYTES: usize = 32;
const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ReportBody {
    version: u16,
    report: AdapterCertificationReport,
}

/// Creates and device-syncs one canonical certification report without replacement.
///
/// # Errors
///
/// Returns serialization, size, I/O, existing-target or digest failures.
pub fn write_report_create_new(
    path: impl AsRef<Path>,
    report: &AdapterCertificationReport,
) -> Result<(), AdapterCertificationReportFileError> {
    if !report.verify_digest() {
        return Err(AdapterCertificationReportFileError::ReportDigest);
    }
    let body = serde_json::to_vec(&ReportBody {
        version: VERSION,
        report: report.clone(),
    })
    .map_err(|error| AdapterCertificationReportFileError::Json(error.to_string()))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(AdapterCertificationReportFileError::Length);
    }
    let body_len =
        u64::try_from(body.len()).map_err(|_| AdapterCertificationReportFileError::Length)?;
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

/// Reads and fully verifies one certification report.
///
/// # Errors
///
/// Rejects malformed, oversized, noncanonical, corrupt or digest-invalid data.
pub fn read_report(
    path: impl AsRef<Path>,
) -> Result<AdapterCertificationReport, AdapterCertificationReportFileError> {
    let bytes = fs::read(path)?;
    if bytes.len() < HEADER_BYTES + CHECKSUM_BYTES || bytes.get(0..8) != Some(MAGIC) {
        return Err(AdapterCertificationReportFileError::Length);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| AdapterCertificationReportFileError::Length)?,
    );
    if version != VERSION {
        return Err(AdapterCertificationReportFileError::Version(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(AdapterCertificationReportFileError::Reserved);
    }
    let body_len = usize::try_from(u64::from_le_bytes(
        bytes[16..24]
            .try_into()
            .map_err(|_| AdapterCertificationReportFileError::Length)?,
    ))
    .map_err(|_| AdapterCertificationReportFileError::Length)?;
    if body_len > MAX_BODY_BYTES || bytes.len() != HEADER_BYTES + body_len + CHECKSUM_BYTES {
        return Err(AdapterCertificationReportFileError::Length);
    }
    let checksum_at = HEADER_BYTES + body_len;
    if blake3::hash(&bytes[..checksum_at]).as_bytes() != &bytes[checksum_at..] {
        return Err(AdapterCertificationReportFileError::Checksum);
    }
    let mut deserializer = serde_json::Deserializer::from_slice(&bytes[HEADER_BYTES..checksum_at]);
    let body = ReportBody::deserialize(&mut deserializer)
        .map_err(|error| AdapterCertificationReportFileError::Json(error.to_string()))?;
    deserializer
        .end()
        .map_err(|error| AdapterCertificationReportFileError::Json(error.to_string()))?;
    if body.version != VERSION {
        return Err(AdapterCertificationReportFileError::Version(body.version));
    }
    let canonical = serde_json::to_vec(&body)
        .map_err(|error| AdapterCertificationReportFileError::Json(error.to_string()))?;
    if canonical != bytes[HEADER_BYTES..checksum_at] {
        return Err(AdapterCertificationReportFileError::NonCanonical);
    }
    if !body.report.verify_digest() {
        return Err(AdapterCertificationReportFileError::ReportDigest);
    }
    Ok(body.report)
}

#[derive(Debug, Error)]
pub enum AdapterCertificationReportFileError {
    #[error("deployment-adapter certification report I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("deployment-adapter certification report JSON is invalid: {0}")]
    Json(String),
    #[error("deployment-adapter certification report length or magic is invalid")]
    Length,
    #[error("unsupported deployment-adapter certification report version: {0}")]
    Version(u16),
    #[error("deployment-adapter certification report reserved bytes are non-zero")]
    Reserved,
    #[error("deployment-adapter certification report checksum is invalid")]
    Checksum,
    #[error("deployment-adapter certification report body is not canonical")]
    NonCanonical,
    #[error("deployment-adapter certification report internal digest is invalid")]
    ReportDigest,
}
