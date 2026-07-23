use crate::CanaryReport;
use serde::{Deserialize, Serialize};
use std::{fs, fs::OpenOptions, io::Write, path::Path};
use thiserror::Error;
const MAGIC: &[u8; 8] = b"POLYMCR1";
const V: u16 = 1;
const H: usize = 24;
const S: usize = 32;
const MAX: usize = 8 * 1024 * 1024;
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Body {
    version: u16,
    report: CanaryReport,
}
/// Writes a create-new report.
/// # Errors
/// Rejects invalid, oversized, existing or I/O failure.
#[allow(clippy::many_single_char_names)]
pub fn write_report_create_new(
    path: impl AsRef<Path>,
    r: &CanaryReport,
) -> Result<(), CanaryReportFileError> {
    if !r.verify_digest() {
        return Err(CanaryReportFileError::Digest);
    }
    let body = serde_json::to_vec(&Body {
        version: V,
        report: r.clone(),
    })
    .map_err(|e| CanaryReportFileError::Json(e.to_string()))?;
    if body.len() > MAX {
        return Err(CanaryReportFileError::Length);
    }
    let n = u64::try_from(body.len()).map_err(|_| CanaryReportFileError::Length)?;
    let mut b = Vec::new();
    b.extend_from_slice(MAGIC);
    b.extend_from_slice(&V.to_le_bytes());
    b.extend_from_slice(&[0; 6]);
    b.extend_from_slice(&n.to_le_bytes());
    b.extend_from_slice(&body);
    let s = blake3::hash(&b);
    b.extend_from_slice(s.as_bytes());
    let mut f = OpenOptions::new().create_new(true).write(true).open(path)?;
    f.write_all(&b)?;
    f.sync_all()?;
    Ok(())
}
/// Reads a canonical report.
/// # Errors
/// Rejects malformed, corrupt, noncanonical or invalid data.
pub fn read_report(path: impl AsRef<Path>) -> Result<CanaryReport, CanaryReportFileError> {
    let b = fs::read(path)?;
    if b.len() < H + S || b.get(..8) != Some(MAGIC) {
        return Err(CanaryReportFileError::Length);
    }
    let v = u16::from_le_bytes(
        b[8..10]
            .try_into()
            .map_err(|_| CanaryReportFileError::Length)?,
    );
    if v != V {
        return Err(CanaryReportFileError::Version(v));
    }
    if b[10..16] != [0; 6] {
        return Err(CanaryReportFileError::Reserved);
    }
    let n = usize::try_from(u64::from_le_bytes(
        b[16..24]
            .try_into()
            .map_err(|_| CanaryReportFileError::Length)?,
    ))
    .map_err(|_| CanaryReportFileError::Length)?;
    if n > MAX || b.len() != H + n + S {
        return Err(CanaryReportFileError::Length);
    }
    let end = H + n;
    if blake3::hash(&b[..end]).as_bytes() != &b[end..] {
        return Err(CanaryReportFileError::Checksum);
    }
    let mut d = serde_json::Deserializer::from_slice(&b[H..end]);
    let body = Body::deserialize(&mut d).map_err(|e| CanaryReportFileError::Json(e.to_string()))?;
    d.end()
        .map_err(|e| CanaryReportFileError::Json(e.to_string()))?;
    if body.version != V {
        return Err(CanaryReportFileError::Version(body.version));
    }
    if serde_json::to_vec(&body).map_err(|e| CanaryReportFileError::Json(e.to_string()))?
        != b[H..end]
    {
        return Err(CanaryReportFileError::NonCanonical);
    }
    if !body.report.verify_digest() {
        return Err(CanaryReportFileError::Digest);
    }
    Ok(body.report)
}
#[derive(Debug, Error)]
pub enum CanaryReportFileError {
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
