//! Bounded Linux process and cgroup memory sampling for controlled experiments.

use std::fs;
use std::io::{BufWriter, Write};
use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemorySample {
    pub(crate) elapsed_ms: u64,
    pub(crate) rss_kib: u64,
    pub(crate) swap_kib: u64,
    pub(crate) hwm_kib: u64,
    pub(crate) cgroup_current_bytes: Option<u64>,
    pub(crate) cgroup_swap_bytes: Option<u64>,
}

#[cfg(any(target_os = "linux", test))]
pub(crate) fn parse_status(input: &str) -> Result<(u64, u64, u64), String> {
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

#[cfg(target_os = "linux")]
pub(crate) fn sample_linux(pid: u32, elapsed_ms: u64) -> Result<MemorySample, String> {
    let status_path = PathBuf::from(format!("/proc/{pid}/status"));
    let status = fs::read_to_string(&status_path)
        .map_err(|error| format!("failed to read {}: {error}", status_path.display()))?;
    let (rss_kib, swap_kib, hwm_kib) = parse_status(&status)?;
    let (cgroup_current_bytes, cgroup_swap_bytes) = cgroup_memory(pid)?;

    Ok(MemorySample {
        elapsed_ms,
        rss_kib,
        swap_kib,
        hwm_kib,
        cgroup_current_bytes,
        cgroup_swap_bytes,
    })
}

#[cfg(not(target_os = "linux"))]
pub(crate) fn sample_linux(_pid: u32, _elapsed_ms: u64) -> Result<MemorySample, String> {
    Err("sample-memory is supported only on Linux".to_string())
}

pub(crate) fn run(
    pid: u32,
    duration_secs: u64,
    interval_ms: u64,
    output: &Path,
) -> Result<(), String> {
    const MAX_DURATION_SECS: u64 = 86_400;
    const MAX_INTERVAL_MS: u64 = 60_000;

    if duration_secs == 0 || duration_secs > MAX_DURATION_SECS {
        return Err(format!("duration-secs must be in 1..={MAX_DURATION_SECS}"));
    }
    if interval_ms == 0 || interval_ms > MAX_INTERVAL_MS {
        return Err(format!("interval-ms must be in 1..={MAX_INTERVAL_MS}"));
    }

    let duration = Duration::from_secs(duration_secs);
    let interval = Duration::from_millis(interval_ms);
    let started = Instant::now();
    let file = fs::File::create(output)
        .map_err(|error| format!("failed to create {}: {error}", output.display()))?;
    let mut writer = BufWriter::new(file);
    let mut warned_cgroup_unavailable = false;

    loop {
        let elapsed_ms = u64::try_from(started.elapsed().as_millis())
            .map_err(|error| format!("elapsed time exceeds the sampler range: {error}"))?;
        let sample = sample_linux(pid, elapsed_ms)?;
        if sample.cgroup_current_bytes.is_none() && !warned_cgroup_unavailable {
            eprintln!(
                "sample-memory: cgroup v2 memory counters are unavailable; cgroup fields are null"
            );
            warned_cgroup_unavailable = true;
        }
        write_sample(&mut writer, &sample)?;
        writer
            .flush()
            .map_err(|error| format!("failed to flush {}: {error}", output.display()))?;

        let elapsed = started.elapsed();
        if elapsed >= duration {
            break;
        }
        std::thread::sleep(interval.min(duration.saturating_sub(elapsed)));
    }

    Ok(())
}

fn write_sample(writer: &mut impl Write, sample: &MemorySample) -> Result<(), String> {
    fn optional_number(value: Option<u64>) -> String {
        value.map_or_else(|| "null".to_string(), |number| number.to_string())
    }

    writeln!(
        writer,
        "{{\"elapsed_ms\":{},\"rss_kib\":{},\"swap_kib\":{},\"hwm_kib\":{},\"cgroup_current_bytes\":{},\"cgroup_swap_bytes\":{}}}",
        sample.elapsed_ms,
        sample.rss_kib,
        sample.swap_kib,
        sample.hwm_kib,
        optional_number(sample.cgroup_current_bytes),
        optional_number(sample.cgroup_swap_bytes)
    )
    .map_err(|error| format!("failed to write memory sample: {error}"))
}

#[cfg(target_os = "linux")]
fn cgroup_memory(pid: u32) -> Result<(Option<u64>, Option<u64>), String> {
    let membership_path = PathBuf::from(format!("/proc/{pid}/cgroup"));
    let membership = fs::read_to_string(&membership_path)
        .map_err(|error| format!("failed to read {}: {error}", membership_path.display()))?;
    let Some(group_path) = membership.lines().find_map(|line| {
        line.strip_prefix("0::")
            .map(|path| PathBuf::from(path.trim()))
    }) else {
        return Ok((None, None));
    };

    let mountinfo = fs::read_to_string("/proc/self/mountinfo")
        .map_err(|error| format!("failed to read /proc/self/mountinfo: {error}"))?;
    let Some(group_directory) = cgroup_mount_directory(&mountinfo, &group_path) else {
        return Ok((None, None));
    };
    let current = read_optional_counter(&group_directory.join("memory.current"))?;
    let swap = read_optional_counter(&group_directory.join("memory.swap.current"))?;
    match (current, swap) {
        (Some(current), Some(swap)) => Ok((Some(current), Some(swap))),
        _ => Ok((None, None)),
    }
}

#[cfg(target_os = "linux")]
fn cgroup_mount_directory(mountinfo: &str, group_path: &Path) -> Option<PathBuf> {
    mountinfo.lines().find_map(|line| {
        let (mount_fields, filesystem_fields) = line.split_once(" - ")?;
        let mut mount_fields = mount_fields.split_whitespace();
        let _mount_id = mount_fields.next()?;
        let _parent_id = mount_fields.next()?;
        let _device = mount_fields.next()?;
        let root = PathBuf::from(unescape_mount_field(mount_fields.next()?));
        let mountpoint = PathBuf::from(unescape_mount_field(mount_fields.next()?));
        if filesystem_fields.split_whitespace().next()? != "cgroup2" {
            return None;
        }
        let relative_path = group_path.strip_prefix(root).ok()?;
        Some(mountpoint.join(relative_path))
    })
}

#[cfg(target_os = "linux")]
fn unescape_mount_field(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'\\' && index + 3 < bytes.len() {
            let escape = &bytes[index + 1..index + 4];
            if escape.iter().all(u8::is_ascii_digit) {
                let octal = escape
                    .iter()
                    .fold(0_u16, |value, digit| value * 8 + u16::from(digit - b'0'));
                if let Ok(byte) = u8::try_from(octal) {
                    decoded.push(byte);
                    index += 4;
                    continue;
                }
            }
        }
        decoded.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

#[cfg(target_os = "linux")]
fn read_optional_counter(path: &Path) -> Result<Option<u64>, String> {
    match fs::read_to_string(path) {
        Ok(value) => value
            .trim()
            .parse::<u64>()
            .map(Some)
            .map_err(|error| format!("invalid cgroup counter at {}: {error}", path.display())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("failed to read {}: {error}", path.display())),
    }
}

#[cfg(test)]
mod tests {
    use super::{parse_status, run};

    #[test]
    fn status_preserves_kib_units() {
        let status = "Name:\tservice\nVmHWM:\t140 kB\nVmRSS:\t100 kB\nVmSwap:\t20 kB\n";

        assert_eq!(parse_status(status), Ok((100, 20, 140)));
    }

    #[test]
    fn missing_status_field_is_error() {
        let status = "VmRSS:\t100 kB\nVmSwap:\t20 kB\n";

        assert!(parse_status(status).is_err());
    }

    #[test]
    fn invalid_number_is_error() {
        let status = "VmRSS:\tnot-a-number kB\nVmSwap:\t20 kB\nVmHWM:\t140 kB\n";

        assert!(parse_status(status).is_err());
    }

    #[test]
    fn zero_interval_is_error_before_output_is_created() {
        let directory = tempfile::tempdir().expect("temporary directory should be available");
        let output = directory.path().join("samples.jsonl");

        assert!(run(std::process::id(), 1, 0, &output).is_err());
        assert!(!output.exists());
    }
}
