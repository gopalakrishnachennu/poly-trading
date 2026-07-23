use crate::BrokerCertificationReport;
use serde::{Deserialize, Serialize};
use std::{
    fs::{self, OpenOptions},
    io::Write,
    path::Path,
};
use thiserror::Error;

const MAGIC: &[u8; 8] = b"POLYCBR1";
const VERSION: u16 = 1;
const HEADER: usize = 24;
const CHECKSUM: usize = 32;
const MAX_BODY: usize = 8 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    version: u16,
    report: BrokerCertificationReport,
}
/// Creates and syncs one report without replacement.
///
/// # Errors
///
/// Returns digest, serialization, size, I/O, or existing-target failures.
pub fn write_report_create_new(
    path: impl AsRef<Path>,
    report: &BrokerCertificationReport,
) -> Result<(), BrokerReportFileError> {
    if !report.verify_digest() {
        return Err(BrokerReportFileError::Digest);
    }
    let body = serde_json::to_vec(&Body {
        version: VERSION,
        report: report.clone(),
    })
    .map_err(|e| BrokerReportFileError::Json(e.to_string()))?;
    if body.len() > MAX_BODY {
        return Err(BrokerReportFileError::Length);
    }
    let length = u64::try_from(body.len()).map_err(|_| BrokerReportFileError::Length)?;
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
/// Reads and verifies one canonical report.
///
/// # Errors
///
/// Rejects malformed, oversized, noncanonical, corrupt, or digest-invalid data.
pub fn read_report(
    path: impl AsRef<Path>,
) -> Result<BrokerCertificationReport, BrokerReportFileError> {
    let bytes = fs::read(path)?;
    if bytes.len() < HEADER + CHECKSUM || bytes.get(..8) != Some(MAGIC) {
        return Err(BrokerReportFileError::Length);
    }
    let version = u16::from_le_bytes(
        bytes[8..10]
            .try_into()
            .map_err(|_| BrokerReportFileError::Length)?,
    );
    if version != VERSION {
        return Err(BrokerReportFileError::Version(version));
    }
    if bytes[10..16] != [0; 6] {
        return Err(BrokerReportFileError::Reserved);
    }
    let length = usize::try_from(u64::from_le_bytes(
        bytes[16..24]
            .try_into()
            .map_err(|_| BrokerReportFileError::Length)?,
    ))
    .map_err(|_| BrokerReportFileError::Length)?;
    if length > MAX_BODY || bytes.len() != HEADER + length + CHECKSUM {
        return Err(BrokerReportFileError::Length);
    }
    let end = HEADER + length;
    if blake3::hash(&bytes[..end]).as_bytes() != &bytes[end..] {
        return Err(BrokerReportFileError::Checksum);
    }
    let mut de = serde_json::Deserializer::from_slice(&bytes[HEADER..end]);
    let body =
        Body::deserialize(&mut de).map_err(|e| BrokerReportFileError::Json(e.to_string()))?;
    de.end()
        .map_err(|e| BrokerReportFileError::Json(e.to_string()))?;
    if body.version != VERSION {
        return Err(BrokerReportFileError::Version(body.version));
    }
    if serde_json::to_vec(&body).map_err(|e| BrokerReportFileError::Json(e.to_string()))?
        != bytes[HEADER..end]
    {
        return Err(BrokerReportFileError::NonCanonical);
    }
    if !body.report.verify_digest() {
        return Err(BrokerReportFileError::Digest);
    }
    Ok(body.report)
}
#[derive(Debug, Error)]
pub enum BrokerReportFileError {
    #[error("broker report I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("broker report JSON error: {0}")]
    Json(String),
    #[error("broker report length or magic invalid")]
    Length,
    #[error("unsupported broker report version: {0}")]
    Version(u16),
    #[error("broker report reserved bytes non-zero")]
    Reserved,
    #[error("broker report checksum invalid")]
    Checksum,
    #[error("broker report noncanonical")]
    NonCanonical,
    #[error("broker report digest invalid")]
    Digest,
}
