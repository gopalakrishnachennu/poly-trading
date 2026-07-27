//! Durable recovery of the paper book (Spec 092 / ADR 0096).
//!
//! The journal is the book: realized results are replayed from it at boot, and
//! open positions are restored from a checkpoint written alongside it. Recovery
//! is all-or-nothing — a corrupt line or checkpoint halts startup rather than
//! producing a book that is quietly missing trades.

use crate::journal::DecisionRecord;
use crate::ledger::{ClosedTrade, PaperLedger};
use crate::types::{DecisionAction, MarketSecondSnapshot, OpenPosition, RiskState};
use serde::{Deserialize, Serialize};
use std::path::Path;
use thiserror::Error;

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum RecoveryError {
    #[error("journal {path} line {line} is unparseable: {detail}")]
    CorruptJournalLine {
        path: String,
        line: usize,
        detail: String,
    },
    #[error("journal {path} could not be read: {detail}")]
    JournalUnreadable { path: String, detail: String },
    #[error("position checkpoint {path} is corrupt: {detail}")]
    CorruptCheckpoint { path: String, detail: String },
    #[error(
        "position checkpoint {path} was written by strategy_version {found}, expected {expected}"
    )]
    CheckpointVersionMismatch {
        path: String,
        found: String,
        expected: String,
    },
    #[error("window policy at {path} is corrupt or invalid: {detail}")]
    CorruptPolicy { path: String, detail: String },
}

/// One open position durably recorded so a restart never abandons it.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CheckpointEntry {
    pub condition_id: String,
    pub position: OpenPosition,
    pub risk: RiskState,
    /// Reference level recorded when this window opened.
    pub window_open_micros: i64,
    /// Last observed second, used to settle if the window ends while we are down.
    pub last_snapshot: MarketSecondSnapshot,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PositionCheckpoint {
    pub strategy_version: String,
    pub written_at_ns: i64,
    pub entries: Vec<CheckpointEntry>,
}

/// What boot recovery reconstructed, reported so operators can audit it.
#[derive(Clone, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
pub struct RecoverySummary {
    pub journal_path: String,
    pub journal_records_read: u64,
    pub closes_replayed: u64,
    /// Terminal records from before Spec 091 that carry no realized P&L. Counted
    /// rather than booked as zero, which would dilute every win rate.
    pub pre_accounting_closes_skipped: u64,
    pub positions_restored: u64,
    pub checkpoint_present: bool,
}

/// Replay one day's journal into a ledger.
///
/// A missing file is not an error — the first run of a day has no journal yet.
///
/// # Errors
///
/// Returns [`RecoveryError::CorruptJournalLine`] for any line that does not
/// deserialize, and [`RecoveryError::JournalUnreadable`] if the file exists but
/// cannot be read. Recovery never skips a line it cannot understand.
pub fn replay_journal(
    path: &Path,
    ledger: &mut PaperLedger,
    summary: &mut RecoverySummary,
) -> Result<(), RecoveryError> {
    summary.journal_path = path.display().to_string();
    if !path.exists() {
        return Ok(());
    }
    let raw = std::fs::read_to_string(path).map_err(|e| RecoveryError::JournalUnreadable {
        path: path.display().to_string(),
        detail: e.to_string(),
    })?;

    for (index, line) in raw.lines().enumerate() {
        if line.trim().is_empty() {
            continue;
        }
        let record: DecisionRecord =
            serde_json::from_str(line).map_err(|e| RecoveryError::CorruptJournalLine {
                path: path.display().to_string(),
                line: index + 1,
                detail: e.to_string(),
            })?;
        summary.journal_records_read = summary.journal_records_read.saturating_add(1);

        if !matches!(record.action, DecisionAction::Exit | DecisionAction::Settle) {
            continue;
        }
        let Some(realized) = record.realized_pnl_micros else {
            summary.pre_accounting_closes_skipped =
                summary.pre_accounting_closes_skipped.saturating_add(1);
            continue;
        };
        ledger.record_close(&ClosedTrade {
            setup_id: record
                .setup_id
                .clone()
                .unwrap_or_else(|| "unattributed".to_owned()),
            book: record.book,
            asset: record.asset,
            window: record.window.clone(),
            realized_pnl_micros: realized,
            outcome: record.outcome,
            observed_at_ns: record.observed_at_ns,
            hold_secs: record.hold_secs.unwrap_or(0),
        });
        summary.closes_replayed = summary.closes_replayed.saturating_add(1);
    }
    Ok(())
}

/// Load the open-position checkpoint, if one exists.
///
/// # Errors
///
/// Returns [`RecoveryError::CorruptCheckpoint`] when the file cannot be read or
/// deserialized, and [`RecoveryError::CheckpointVersionMismatch`] when it was
/// written by a different engine version. A financial book is never partially
/// recovered.
pub fn load_checkpoint(
    path: &Path,
    expected_version: &str,
) -> Result<Option<PositionCheckpoint>, RecoveryError> {
    if !path.exists() {
        return Ok(None);
    }
    let raw = std::fs::read_to_string(path).map_err(|e| RecoveryError::CorruptCheckpoint {
        path: path.display().to_string(),
        detail: e.to_string(),
    })?;
    let checkpoint: PositionCheckpoint =
        serde_json::from_str(&raw).map_err(|e| RecoveryError::CorruptCheckpoint {
            path: path.display().to_string(),
            detail: e.to_string(),
        })?;
    if checkpoint.strategy_version != expected_version {
        return Err(RecoveryError::CheckpointVersionMismatch {
            path: path.display().to_string(),
            found: checkpoint.strategy_version,
            expected: expected_version.to_owned(),
        });
    }
    Ok(Some(checkpoint))
}

/// Atomically replace the open-position checkpoint.
///
/// # Errors
///
/// Returns the underlying IO error string. The caller must treat this as fatal:
/// an open position that is not checkpointed is one a restart would abandon.
pub fn write_checkpoint(path: &Path, checkpoint: &PositionCheckpoint) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let tmp = path.with_extension("json.tmp");
    let payload = serde_json::to_vec_pretty(checkpoint).map_err(|e| e.to_string())?;
    std::fs::write(&tmp, payload).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, path).map_err(|e| e.to_string())
}
