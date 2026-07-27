//! Single-writer lock over the paper book (Spec 092 / ADR 0096).
//!
//! Two workers sharing one journal directory silently destroy the book: each
//! holds its own positions in memory, both append to the same journal, and the
//! position checkpoint becomes last-writer-wins, so one worker's open position
//! is erased by the other's empty checkpoint. That is how a funded, locked
//! position was abandoned with no exit and no settlement record.
//!
//! The lock is a pid file. A live holder refuses the start; a dead holder's lock
//! is stale and is taken over, so a crashed worker cannot wedge the book.

use std::path::{Path, PathBuf};
use thiserror::Error;

#[derive(Clone, Debug, Error, Eq, PartialEq)]
pub enum LockError {
    #[error("another btc-window-paper worker (pid {pid}) already owns {path}")]
    Held { path: String, pid: String },
    #[error("could not acquire {path}: {detail}")]
    Unavailable { path: String, detail: String },
}

/// Exclusive ownership of one journal directory. Released on drop.
#[derive(Debug)]
pub struct BookLock {
    path: PathBuf,
}

impl BookLock {
    /// Take exclusive ownership of `journal_dir`.
    ///
    /// # Errors
    ///
    /// Returns [`LockError::Held`] when a live worker already owns the book, and
    /// [`LockError::Unavailable`] when the lock file cannot be written.
    pub fn acquire(journal_dir: &Path) -> Result<Self, LockError> {
        let path = journal_dir.join("writer.lock");
        std::fs::create_dir_all(journal_dir).map_err(|e| LockError::Unavailable {
            path: path.display().to_string(),
            detail: e.to_string(),
        })?;

        if let Ok(existing) = std::fs::read_to_string(&path) {
            let holder = existing.trim().to_owned();
            if !holder.is_empty() && pid_is_alive(&holder) {
                return Err(LockError::Held {
                    path: path.display().to_string(),
                    pid: holder,
                });
            }
            // Stale: the previous owner died without releasing. Take it over so a
            // crash cannot permanently wedge the book.
        }

        std::fs::write(&path, std::process::id().to_string()).map_err(|e| {
            LockError::Unavailable {
                path: path.display().to_string(),
                detail: e.to_string(),
            }
        })?;
        Ok(Self { path })
    }
}

impl Drop for BookLock {
    fn drop(&mut self) {
        // Only clear the lock if we still own it, so a taken-over stale lock is
        // never removed from under its new owner.
        if let Ok(holder) = std::fs::read_to_string(&self.path) {
            if holder.trim() == std::process::id().to_string() {
                let _ = std::fs::remove_file(&self.path);
            }
        }
    }
}

/// Ask the OS whether a pid is still running.
///
/// Uses `kill -0` rather than `libc::kill`, because unsafe Rust is forbidden in
/// this workspace without an accepted ADR isolating it.
fn pid_is_alive(pid: &str) -> bool {
    if pid.parse::<u32>().is_err() {
        return false;
    }
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .is_ok_and(|status| status.success())
}
