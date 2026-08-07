//! Per-revision filesystem lease coordination.
//!
//! Concurrent processes coordinate through a per-extractor/revision lease
//! file created with atomic standard-library file creation. The owner records
//! identity, process, timestamps, and heartbeat. Waiters observe activation
//! rather than duplicating downloads. Stale leases are reclaimed only when the
//! heartbeat is expired AND the same-host process liveness check fails;
//! otherwise waiters wait and report progress.

use std::path::{Path, PathBuf};

use crate::service::MemoryError;

/// Maximum lease age without a heartbeat before a liveness check is attempted.
pub(crate) const LEASE_HEARTBEAT_TTL_SECS: i64 = 90;

/// The lease file format persisted as JSON.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct LeaseRecord {
    /// Extractor identity the lease is for.
    pub extractor: String,
    /// Revision the lease is for.
    pub revision: String,
    /// Owning process PID.
    pub pid: u32,
    /// Unix epoch seconds at creation.
    pub created_at: i64,
    /// Unix epoch seconds of the last heartbeat.
    pub heartbeat_at: i64,
    /// Process-unique staging path used by the owner.
    pub staging: PathBuf,
}

/// A held lease. Dropping it releases the file.
#[derive(Debug)]
pub struct Lease {
    path: PathBuf,
}

impl Lease {
    /// Returns the recorded lease for a revision path, if any.
    pub fn read(lease_path: &Path) -> Result<Option<LeaseRecord>, MemoryError> {
        let bytes = match std::fs::read(lease_path) {
            Ok(bytes) => bytes,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(err) => {
                return Err(MemoryError::Storage(format!(
                    "cannot read lease {}: {err}",
                    lease_path.display()
                )));
            }
        };
        let record = serde_json::from_slice(&bytes).map_err(|err| {
            MemoryError::Storage(format!("invalid lease {}: {err}", lease_path.display()))
        })?;
        Ok(Some(record))
    }

    /// Atomically acquires the lease, returning `None` when another process
    /// holds it and is considered live.
    pub fn acquire(lease_path: &Path, record: &LeaseRecord) -> Result<Option<Lease>, MemoryError> {
        if let Some(parent) = lease_path.parent() {
            std::fs::create_dir_all(parent).map_err(|err| {
                MemoryError::Storage(format!(
                    "cannot create lease directory {}: {err}",
                    parent.display()
                ))
            })?;
        }
        let json = serde_json::to_vec(record)
            .map_err(|err| MemoryError::Storage(format!("cannot serialize lease: {err}")))?;
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        let file = match options.open(lease_path) {
            Ok(file) => file,
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => return Ok(None),
            Err(err) => {
                return Err(MemoryError::Storage(format!(
                    "cannot acquire lease {}: {err}",
                    lease_path.display()
                )));
            }
        };
        use std::io::Write;
        if let Err(err) = std::io::BufWriter::new(file).write_all(&json) {
            let _ = std::fs::remove_file(lease_path);
            return Err(MemoryError::Storage(format!(
                "cannot write lease {}: {err}",
                lease_path.display()
            )));
        }
        Ok(Some(Lease {
            path: lease_path.to_path_buf(),
        }))
    }

    /// Refreshes the heartbeat timestamp.
    pub fn heartbeat(&self, now: i64) -> Result<(), MemoryError> {
        let record = Self::read(&self.path)?.ok_or_else(|| {
            MemoryError::Storage(format!(
                "lease {} disappeared while held",
                self.path.display()
            ))
        })?;
        let updated = LeaseRecord {
            heartbeat_at: now,
            ..record
        };
        let json = serde_json::to_vec(&updated)
            .map_err(|err| MemoryError::Storage(format!("cannot serialize lease: {err}")))?;
        std::fs::write(&self.path, json).map_err(|err| {
            MemoryError::Storage(format!(
                "cannot heartbeat lease {}: {err}",
                self.path.display()
            ))
        })?;
        Ok(())
    }
}

impl Drop for Lease {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

/// Conservative same-host process liveness check.
///
/// Uses `kill -0` on Unix and `tasklist` on Windows. Any tool failure is
/// treated as "unknown" (`None`), which callers must interpret as
/// wait-instead-of-reclaim.
pub fn process_is_live(pid: u32) -> Option<bool> {
    #[cfg(unix)]
    {
        let status = std::process::Command::new("kill")
            .arg("-0")
            .arg(pid.to_string())
            .status()
            .ok()?;
        Some(status.success())
    }
    #[cfg(windows)]
    {
        let output = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {pid}"), "/NH"])
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        Some(stdout.contains(&pid.to_string()))
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

/// Decides whether a stale lease may be reclaimed.
///
/// Never reclaims solely by age: the heartbeat must be expired AND the owner
/// process must be confirmed dead. Unknown liveness means "wait".
#[must_use]
pub fn can_reclaim(record: &LeaseRecord, now: i64) -> bool {
    if now - record.heartbeat_at < LEASE_HEARTBEAT_TTL_SECS {
        return false;
    }
    match process_is_live(record.pid) {
        Some(false) => true,
        // Live owner, or liveness unknown: be conservative.
        Some(true) | None => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn record(pid: u32, heartbeat_at: i64) -> LeaseRecord {
        LeaseRecord {
            extractor: "vago".to_string(),
            revision: "abc123".to_string(),
            pid,
            created_at: heartbeat_at - 10,
            heartbeat_at,
            staging: PathBuf::from("/tmp/staging"),
        }
    }

    #[test]
    fn acquire_is_exclusive_and_drop_releases() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("lease.json");
        let first = Lease::acquire(&path, &record(100, 1_700_000_000))
            .expect("acquire")
            .expect("first owner");
        assert!(
            Lease::acquire(&path, &record(200, 1_700_000_000))
                .expect("acquire")
                .is_none()
        );
        drop(first);
        assert!(
            Lease::acquire(&path, &record(200, 1_700_000_000))
                .expect("acquire")
                .is_some()
        );
    }

    #[test]
    fn heartbeat_updates_record() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("lease.json");
        let lease = Lease::acquire(&path, &record(100, 1_700_000_000))
            .expect("acquire")
            .expect("owner");
        lease.heartbeat(1_700_000_100).expect("heartbeat");
        let loaded = Lease::read(&path).expect("read").expect("record");
        assert_eq!(loaded.heartbeat_at, 1_700_000_100);
    }

    #[test]
    fn can_reclaim_only_after_expired_heartbeat_and_dead_process() {
        // Fresh heartbeat: never reclaim.
        assert!(!can_reclaim(
            &record(std::process::id(), 1_700_000_000),
            1_700_000_000
        ));
        // Expired heartbeat against the live current process: never reclaim.
        assert!(!can_reclaim(
            &record(
                std::process::id(),
                1_700_000_000 - LEASE_HEARTBEAT_TTL_SECS - 1
            ),
            1_700_000_000
        ));
        // Expired heartbeat with a definitely-dead PID: reclaim.
        assert!(can_reclaim(
            &record(999_999, 1_700_000_000 - LEASE_HEARTBEAT_TTL_SECS - 1),
            1_700_000_000
        ));
    }

    #[test]
    fn missing_lease_reads_as_none() {
        let dir = TempDir::new().expect("temp dir");
        assert!(
            Lease::read(&dir.path().join("absent.json"))
                .expect("read")
                .is_none()
        );
    }
}
