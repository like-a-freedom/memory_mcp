//! Resource sampling: Linux `/proc`, macOS `proc_pid_rusage` (ADR-0082).
//!
//! Unsupported platforms get a sampler that always reports `None`; the runner
//! turns that into an `invalid_reason`, never into zeros.

use crate::perf::domain::ResourceSnapshot;

/// A process-resource probe; `None` means "platform cannot measure this".
pub trait ResourceSampler: Send + Sync {
    fn sample(&self) -> Option<ResourceSnapshot>;
}

/// Sampler for platforms without a backend: always `None`.
pub struct UnsupportedSampler;

impl ResourceSampler for UnsupportedSampler {
    fn sample(&self) -> Option<ResourceSnapshot> {
        None
    }
}

#[cfg(target_os = "linux")]
pub struct LinuxProcSampler;

#[cfg(target_os = "linux")]
impl ResourceSampler for LinuxProcSampler {
    fn sample(&self) -> Option<ResourceSnapshot> {
        let status = std::fs::read_to_string("/proc/self/status").ok()?;
        let io = std::fs::read_to_string("/proc/self/io").ok()?;
        let (rss_kib, _swap_kib, hwm_kib) = parse_proc_status(&status).ok()?;
        let (read_bytes, write_bytes, syscr, syscw) = parse_proc_io(&io).ok()?;
        Some(ResourceSnapshot {
            rss_bytes: Some(rss_kib * 1024),
            peak_rss_bytes: Some(hwm_kib * 1024),
            disk_read_bytes: Some(read_bytes),
            disk_write_bytes: Some(write_bytes),
            io_read_ops: Some(syscr),
            io_write_ops: Some(syscw),
        })
    }
}

#[cfg(target_os = "macos")]
pub struct MacRusageSampler;

#[cfg(target_os = "macos")]
impl ResourceSampler for MacRusageSampler {
    fn sample(&self) -> Option<ResourceSnapshot> {
        let mut info: libc::rusage_info_v2 = unsafe { std::mem::zeroed() };
        let rc = unsafe {
            libc::proc_pid_rusage(
                std::process::id() as libc::c_int,
                libc::RUSAGE_INFO_V2,
                &mut info as *mut libc::rusage_info_v2 as *mut libc::rusage_info_t,
            )
        };
        if rc != 0 {
            return None;
        }
        Some(ResourceSnapshot {
            rss_bytes: Some(info.ri_phys_footprint),
            peak_rss_bytes: Some(info.ri_resident_size),
            disk_read_bytes: Some(info.ri_diskio_bytesread),
            disk_write_bytes: Some(info.ri_diskio_byteswritten),
            io_read_ops: None,
            io_write_ops: None,
        })
    }
}

/// The best sampler this platform has; `UnsupportedSampler` elsewhere.
pub fn self_sampler() -> Box<dyn ResourceSampler> {
    #[cfg(target_os = "linux")]
    {
        return Box::new(LinuxProcSampler);
    }
    #[cfg(target_os = "macos")]
    {
        return Box::new(MacRusageSampler);
    }
    #[allow(unreachable_code)]
    Box::new(UnsupportedSampler)
}

/// Parse `VmRSS`, `VmSwap`, `VmHWM` from a `/proc/*/status` dump, in kB as
/// written (the caller converts to bytes).
pub fn parse_proc_status(input: &str) -> Result<(u64, u64, u64), String> {
    fn parse_kib(input: &str, field: &str) -> Result<u64, String> {
        let line = input
            .lines()
            .find(|line| {
                line.split_once(':')
                    .is_some_and(|(name, _)| name.trim() == field)
            })
            .ok_or_else(|| format!("{field} is missing from /proc status"))?;
        let value = line
            .split_once(':')
            .map(|(_, value)| value.trim())
            .ok_or_else(|| format!("{field} has no value separator"))?;
        let mut parts = value.split_whitespace();
        let number = parts
            .next()
            .ok_or_else(|| format!("{field} has no value"))?
            .parse::<u64>()
            .map_err(|error| format!("{field} is not a valid integer: {error}"))?;
        match parts.next() {
            Some("kB") if parts.next().is_none() => Ok(number),
            _ => Err(format!("{field} is not expressed in kB")),
        }
    }

    Ok((
        parse_kib(input, "VmRSS")?,
        parse_kib(input, "VmSwap")?,
        parse_kib(input, "VmHWM")?,
    ))
}

/// Parse `read_bytes`, `write_bytes`, `syscr`, `syscw` from `/proc/*/io`.
pub fn parse_proc_io(input: &str) -> Result<(u64, u64, u64, u64), String> {
    fn parse_u64(input: &str, field: &str) -> Result<u64, String> {
        let line = input
            .lines()
            .find(|line| {
                line.split_once(':')
                    .is_some_and(|(name, _)| name.trim() == field)
            })
            .ok_or_else(|| format!("{field} is missing from /proc io"))?;
        let value = line
            .split_once(':')
            .map(|(_, value)| value.trim())
            .ok_or_else(|| format!("{field} has no value separator"))?;
        value
            .parse::<u64>()
            .map_err(|error| format!("{field} is not a valid integer: {error}"))
    }

    Ok((
        parse_u64(input, "read_bytes")?,
        parse_u64(input, "write_bytes")?,
        parse_u64(input, "syscr")?,
        parse_u64(input, "syscw")?,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    const STATUS_FIXTURE: &str =
        "Name:\tmemory-perf\nVmRSS:\t    2048 kB\nVmSwap:\t       0 kB\nVmHWM:\t    4096 kB\n";

    const IO_FIXTURE: &str = "rchar: 100\nwchar: 200\nsyscr: 7\nsyscw: 9\nread_bytes: 4096\nwrite_bytes: 8192\ncancelled_write_bytes: 0\n";

    #[test]
    fn parse_proc_status_returns_rss_swap_hwm_in_kib() {
        let (rss, swap, hwm) = parse_proc_status(STATUS_FIXTURE).expect("status parses");
        assert_eq!(rss, 2048);
        assert_eq!(swap, 0);
        assert_eq!(hwm, 4096);
    }

    #[test]
    fn parse_proc_status_rejects_missing_field() {
        let err = parse_proc_status("Name:\tx\n").expect_err("VmRSS missing");
        assert!(err.contains("VmRSS"), "got {err}");
    }

    #[test]
    fn parse_proc_io_returns_disk_bytes_and_op_counts() {
        let (read, written, syscr, syscw) = parse_proc_io(IO_FIXTURE).expect("io parses");
        assert_eq!(read, 4096);
        assert_eq!(written, 8192);
        assert_eq!(syscr, 7);
        assert_eq!(syscw, 9);
    }

    #[test]
    fn parse_proc_io_rejects_missing_field() {
        let err = parse_proc_io("rchar: 1\n").expect_err("read_bytes missing");
        assert!(err.contains("read_bytes"), "got {err}");
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    #[test]
    fn self_sampler_reports_rss_on_supported_platform() {
        let sampler = self_sampler();
        let snapshot = sampler
            .sample()
            .expect("sample must succeed on linux/macos");
        assert!(
            snapshot.rss_bytes.unwrap_or(0) > 0,
            "rss_bytes must be Some and positive, got {snapshot:?}"
        );
        assert!(snapshot.peak_rss_bytes.unwrap_or(0) > 0, "got {snapshot:?}");
    }

    #[cfg(target_os = "macos")]
    #[tokio::test]
    async fn mac_sampler_reports_disk_writes() {
        use std::io::Write;

        let sampler = self_sampler();
        let before = sampler.sample().expect("sample before write");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("payload.bin");
        let mut file = std::fs::File::create(&path).expect("create");
        file.write_all(&vec![0u8; 1 << 20]).expect("write 1 MiB");
        file.sync_all().expect("fsync");
        let after = sampler.sample().expect("sample after write");
        let delta = after
            .disk_write_bytes
            .unwrap_or(0)
            .saturating_sub(before.disk_write_bytes.unwrap_or(0));
        assert!(
            delta > 0,
            "disk_write_bytes must grow after fsync: {before:?} -> {after:?}"
        );
        assert!(after.rss_bytes.unwrap_or(0) > (1 << 20), "rss above 1 MiB");
    }

    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn linux_sampler_reports_disk_writes() {
        use std::io::Write;

        let sampler = self_sampler();
        let before = sampler.sample().expect("sample before write");
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("payload.bin");
        let mut file = std::fs::File::create(&path).expect("create");
        file.write_all(&vec![0u8; 1 << 20]).expect("write 1 MiB");
        file.sync_all().expect("fsync");
        let after = sampler.sample().expect("sample after write");
        let delta = after
            .disk_write_bytes
            .unwrap_or(0)
            .saturating_sub(before.disk_write_bytes.unwrap_or(0));
        assert!(
            delta > 0,
            "disk_write_bytes must grow after fsync: {before:?} -> {after:?}"
        );
        assert!(after.rss_bytes.unwrap_or(0) > (1 << 20), "rss above 1 MiB");
    }
}
