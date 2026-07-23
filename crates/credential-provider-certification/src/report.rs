use crate::ProviderCertificationReport;
use serde::{Deserialize, Serialize};
use std::{fs, fs::OpenOptions, io::Write, path::Path};
use thiserror::Error;

const MAGIC: &[u8; 8] = b"POLYPCR1";
const VERSION: u16 = 1;
const HEADER_BYTES: usize = 24;
const CHECKSUM_BYTES: usize = 32;
const MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    version: u16,
    report: ProviderCertificationReport,
}

/// Creates and syncs one canonical report without replacement.
///
/// # Errors
///
/// Returns digest, serialization, size, I/O, or existing-target failures.
pub fn write_report_create_new(
    path: impl AsRef<Path>,
    report: &ProviderCertificationReport,
) -> Result<(), ProviderReportFileError> {
    if !report.verify_digest() {
        return Err(ProviderReportFileError::Digest);
    }
    let body = serde_json::to_vec(&Body {
        version: VERSION,
        report: report.clone(),
    })
    .map_err(|error| ProviderReportFileError::Json(error.to_string()))?;
    if body.len() > MAX_BODY_BYTES {
        return Err(ProviderReportFileError::Length);
    }
    let length = u64::try_from(body.len()).map_err(|_| ProviderReportFileError::Length)?;
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
) -> Result<ProviderCertificationReport, ProviderReportFileError> {
    let bytes = fs::read(path)?;
    if bytes.len() < HEADER_BYTES + CHECKSUM_BYTES || bytes.get(..8) != Some(MAGIC) {
        return Err(ProviderReportFileError::Length);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| ProviderReportFileError::Length)?,
    );
    if version != VERSION {
        return Err(ProviderReportFileError::Version(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(ProviderReportFileError::Reserved);
    }
    let length = usize::try_from(u64::from_le_bytes(
        bytes[16..24]
            .try_into()
            .map_err(|_| ProviderReportFileError::Length)?,
    ))
    .map_err(|_| ProviderReportFileError::Length)?;
    if length > MAX_BODY_BYTES || bytes.len() != HEADER_BYTES + length + CHECKSUM_BYTES {
        return Err(ProviderReportFileError::Length);
    }
    let end = HEADER_BYTES + length;
    if blake3::hash(&bytes[..end]).as_bytes() != &bytes[end..] {
        return Err(ProviderReportFileError::Checksum);
    }
    let mut decoder = serde_json::Deserializer::from_slice(&bytes[HEADER_BYTES..end]);
    let body = Body::deserialize(&mut decoder)
        .map_err(|error| ProviderReportFileError::Json(error.to_string()))?;
    decoder
        .end()
        .map_err(|error| ProviderReportFileError::Json(error.to_string()))?;
    if body.version != VERSION {
        return Err(ProviderReportFileError::Version(body.version));
    }
    if serde_json::to_vec(&body)
        .map_err(|error| ProviderReportFileError::Json(error.to_string()))?
        != bytes[HEADER_BYTES..end]
    {
        return Err(ProviderReportFileError::NonCanonical);
    }
    if !body.report.verify_digest() {
        return Err(ProviderReportFileError::Digest);
    }
    Ok(body.report)
}

#[derive(Debug, Error)]
pub enum ProviderReportFileError {
    #[error("provider certification report I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("provider certification report JSON error: {0}")]
    Json(String),
    #[error("provider certification report length or magic invalid")]
    Length,
    #[error("unsupported provider certification report version: {0}")]
    Version(u16),
    #[error("provider certification report reserved bytes non-zero")]
    Reserved,
    #[error("provider certification report checksum invalid")]
    Checksum,
    #[error("provider certification report noncanonical")]
    NonCanonical,
    #[error("provider certification report digest invalid")]
    Digest,
}
