//! Per-revision filesystem lease coordination.
//!
//! Concurrent processes coordinate through a per-extractor/revision lease
//! file created with atomic standard-library file creation. The owner records
//! identity, process, timestamps, and heartbeat. Waiters observe activation
//! rather than duplicating downloads. Stale leases are reclaimed only when the
//! heartbeat is expired AND the same-host process liveness check fails;
//! otherwise waiters wait and report progress.

use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::time::{Duration, Instant};

use crate::error::MemoryError;

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
/// wait-instead-of-reclaim. Unix permission errors do not prove death;
/// only the standard missing-process diagnostic can authorize reclamation.
pub fn process_is_live(pid: u32) -> Option<bool> {
    #[cfg(unix)]
    {
        let output = bounded_probe(
            Command::new("/bin/kill")
                .env("LC_ALL", "C")
                .arg("-0")
                .arg(pid.to_string()),
        )?;
        if output.status.success() {
            Some(true)
        } else if String::from_utf8_lossy(&output.stderr).contains("No such process") {
            Some(false)
        } else {
            None
        }
    }
    #[cfg(windows)]
    {
        let output =
            bounded_probe(Command::new("tasklist").args(["/FI", &format!("PID eq {pid}"), "/NH"]))?;
        if !output.status.success() {
            return None;
        }
        let stdout = String::from_utf8_lossy(&output.stdout);
        if stdout
            .split_whitespace()
            .any(|field| field == pid.to_string())
        {
            Some(true)
        } else if stdout.contains("No tasks are running") {
            Some(false)
        } else {
            None
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        None
    }
}

fn bounded_probe(command: &mut Command) -> Option<Output> {
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .ok()?;
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        match child.try_wait() {
            Ok(Some(_)) => return child.wait_with_output().ok(),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) | Err(_) => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

/// Decides whether a stale lease may be reclaimed.
///
/// Never reclaims solely by age: the heartbeat must be expired AND the owner
/// process must be confirmed dead. Unknown liveness means "wait".
#[must_use]
pub fn can_reclaim(
    record: &LeaseRecord,
    now: i64,
    process_is_live: impl FnOnce(u32) -> Option<bool>,
) -> bool {
    if now.saturating_sub(record.heartbeat_at) < LEASE_HEARTBEAT_TTL_SECS {
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

    fn record(heartbeat_at: i64) -> LeaseRecord {
        LeaseRecord {
            extractor: "vago".to_string(),
            revision: "abc123".to_string(),
            pid: 77,
            created_at: heartbeat_at - 10,
            heartbeat_at,
            staging: PathBuf::from("/tmp/staging"),
        }
    }

    // Integration: the real platform probe sees the current live process.
    #[test]
    fn platform_probe_confirms_a_live_process() {
        assert_eq!(process_is_live(std::process::id()), Some(true));
    }

    // Integration: the OS has reaped this child before liveness is checked.
    #[cfg(unix)]
    #[test]
    fn platform_probe_confirms_a_reaped_process_is_dead() {
        let mut child = Command::new("/bin/sh")
            .args(["-c", "exit 0"])
            .spawn()
            .expect("spawn bounded child");
        let pid = child.id();
        child.wait().expect("reap child");
        assert_eq!(process_is_live(pid), Some(false));
    }

    // Integration evidence: these cases exercise real temporary lease files.
    #[test]
    fn acquire_is_exclusive_and_drop_releases() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("lease.json");
        let first = Lease::acquire(&path, &record(1_700_000_000))
            .expect("acquire")
            .expect("first owner");
        assert!(
            Lease::acquire(&path, &record(1_700_000_000))
                .expect("acquire")
                .is_none()
        );
        drop(first);
        assert!(
            Lease::acquire(&path, &record(1_700_000_000))
                .expect("acquire")
                .is_some()
        );
    }

    #[test]
    fn heartbeat_updates_record() {
        let dir = TempDir::new().expect("temp dir");
        let path = dir.path().join("lease.json");
        let lease = Lease::acquire(&path, &record(1_700_000_000))
            .expect("acquire")
            .expect("owner");
        lease.heartbeat(1_700_000_100).expect("heartbeat");
        let loaded = Lease::read(&path).expect("read").expect("record");
        assert_eq!(loaded.heartbeat_at, 1_700_000_100);
    }

    // Unit policy cases below receive controlled liveness and never spawn tools.
    #[test]
    fn fresh_heartbeat_skips_process_liveness_check() {
        assert!(!can_reclaim(&record(1_700_000_000), 1_700_000_000, |_| {
            panic!("fresh lease must not invoke process liveness")
        }));
    }

    #[test]
    fn expired_heartbeat_is_not_reclaimed_while_process_is_live() {
        assert!(!can_reclaim(
            &record(1_700_000_000 - LEASE_HEARTBEAT_TTL_SECS - 1),
            1_700_000_000,
            |_| Some(true)
        ));
    }

    #[test]
    fn expired_heartbeat_is_reclaimed_after_process_death_is_confirmed() {
        let owner = record(1_700_000_000 - LEASE_HEARTBEAT_TTL_SECS - 1);

        assert!(can_reclaim(&owner, 1_700_000_000, |pid| {
            (pid == 77).then_some(false)
        }));
    }

    #[test]
    fn expired_heartbeat_is_not_reclaimed_when_process_liveness_is_unknown() {
        assert!(!can_reclaim(
            &record(1_700_000_000 - LEASE_HEARTBEAT_TTL_SECS - 1),
            1_700_000_000,
            |_| None
        ));
    }

    #[test]
    fn a_future_heartbeat_is_not_reclaimed_at_the_clock_range_boundary() {
        let owner = record(i64::MAX);

        assert!(!can_reclaim(&owner, i64::MIN, |_| Some(false)));
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
