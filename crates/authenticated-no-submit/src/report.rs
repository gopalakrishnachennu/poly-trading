use crate::AuthReport;
use serde::{Deserialize, Serialize};
use std::{fs, fs::OpenOptions, io::Write, path::Path};
use thiserror::Error;
const MAGIC: &[u8; 8] = b"POLYANR1";
const VERSION: u16 = 1;
const HEADER: usize = 24;
const SUM: usize = 32;
const MAX: usize = 8 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    version: u16,
    report: AuthReport,
}
/// Creates one canonical synced report without replacement.
/// # Errors
/// Rejects invalid, oversized, unserializable or existing output.
pub fn write_report_create_new(
    path: impl AsRef<Path>,
    report: &AuthReport,
) -> Result<(), AuthReportFileError> {
    if !report.verify_digest() {
        return Err(AuthReportFileError::Digest);
    }
    let body = serde_json::to_vec(&Body {
        version: VERSION,
        report: report.clone(),
    })
    .map_err(|e| AuthReportFileError::Json(e.to_string()))?;
    if body.len() > MAX {
        return Err(AuthReportFileError::Length);
    }
    let n = u64::try_from(body.len()).map_err(|_| AuthReportFileError::Length)?;
    let mut b = Vec::with_capacity(HEADER + body.len() + SUM);
    b.extend_from_slice(MAGIC);
    b.extend_from_slice(&VERSION.to_le_bytes());
    b.extend_from_slice(&[0; 6]);
    b.extend_from_slice(&n.to_le_bytes());
    b.extend_from_slice(&body);
    let sum = blake3::hash(&b);
    b.extend_from_slice(sum.as_bytes());
    let mut f = OpenOptions::new().create_new(true).write(true).open(path)?;
    f.write_all(&b)?;
    f.sync_all()?;
    Ok(())
}
/// Reads and verifies one canonical report.
/// # Errors
/// Rejects malformed, oversized, noncanonical, corrupt or digest-invalid data.
pub fn read_report(path: impl AsRef<Path>) -> Result<AuthReport, AuthReportFileError> {
    let b = fs::read(path)?;
    if b.len() < HEADER + SUM || b.get(..8) != Some(MAGIC) {
        return Err(AuthReportFileError::Length);
    }
    let v = u16::from_le_bytes(
        b[8..10]
            .try_into()
            .map_err(|_| AuthReportFileError::Length)?,
    );
    if v != VERSION {
        return Err(AuthReportFileError::Version(v));
    }
    if b[10..16] != [0; 6] {
        return Err(AuthReportFileError::Reserved);
    }
    let n = usize::try_from(u64::from_le_bytes(
        b[16..24]
            .try_into()
            .map_err(|_| AuthReportFileError::Length)?,
    ))
    .map_err(|_| AuthReportFileError::Length)?;
    if n > MAX || b.len() != HEADER + n + SUM {
        return Err(AuthReportFileError::Length);
    }
    let end = HEADER + n;
    if blake3::hash(&b[..end]).as_bytes() != &b[end..] {
        return Err(AuthReportFileError::Checksum);
    }
    let mut d = serde_json::Deserializer::from_slice(&b[HEADER..end]);
    let body = Body::deserialize(&mut d).map_err(|e| AuthReportFileError::Json(e.to_string()))?;
    d.end()
        .map_err(|e| AuthReportFileError::Json(e.to_string()))?;
    if body.version != VERSION {
        return Err(AuthReportFileError::Version(body.version));
    }
    if serde_json::to_vec(&body).map_err(|e| AuthReportFileError::Json(e.to_string()))?
        != b[HEADER..end]
    {
        return Err(AuthReportFileError::NonCanonical);
    }
    if !body.report.verify_digest() {
        return Err(AuthReportFileError::Digest);
    }
    Ok(body.report)
}
#[derive(Debug, Error)]
pub enum AuthReportFileError {
    #[error("auth report I/O: {0}")]
    Io(#[from] std::io::Error),
    #[error("auth report JSON: {0}")]
    Json(String),
    #[error("auth report length or magic")]
    Length,
    #[error("auth report version {0}")]
    Version(u16),
    #[error("auth report reserved bytes")]
    Reserved,
    #[error("auth report checksum")]
    Checksum,
    #[error("auth report noncanonical")]
    NonCanonical,
    #[error("auth report digest")]
    Digest,
}
