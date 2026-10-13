//! Bounded Linux process and cgroup memory sampling for controlled experiments.

use std::fs;
use std::io::{self, BufWriter, Write};
use std::path::Path;
#[cfg(target_os = "linux")]
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MemorySample {
    pub(crate) binary_name: String,
    pub(crate) process_alive: bool,
    pub(crate) elapsed_ms: u64,
    pub(crate) rss_kib: u64,
    pub(crate) swap_kib: u64,
    pub(crate) hwm_kib: u64,
    pub(crate) cgroup_current_bytes: Option<u64>,
    pub(crate) cgroup_swap_bytes: Option<u64>,
    pub(crate) cgroup_memory_peak_bytes: Option<u64>,
    pub(crate) cgroup_oom: Option<u64>,
    pub(crate) cgroup_oom_kill: Option<u64>,
    #[cfg(target_os = "linux")]
    cgroup_directory: Option<PathBuf>,
}

#[cfg(target_os = "linux")]
#[derive(Debug, Default)]
struct CgroupMemory {
    current_bytes: Option<u64>,
    swap_bytes: Option<u64>,
    peak_bytes: Option<u64>,
    oom: Option<u64>,
    oom_kill: Option<u64>,
    directory: Option<PathBuf>,
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
    let executable_path = PathBuf::from(format!("/proc/{pid}/exe"));
    let executable = fs::read_link(&executable_path)
        .map_err(|error| format!("failed to read {}: {error}", executable_path.display()))?;
    let binary_name = executable
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("{} has no UTF-8 executable name", executable_path.display()))?
        .to_string();
    let status_path = PathBuf::from(format!("/proc/{pid}/status"));
    let status = fs::read_to_string(&status_path)
        .map_err(|error| format!("failed to read {}: {error}", status_path.display()))?;
    let (rss_kib, swap_kib, hwm_kib) = parse_status(&status)?;
    let cgroup = cgroup_memory(pid)?;

    Ok(MemorySample {
        binary_name,
        process_alive: true,
        elapsed_ms,
        rss_kib,
        swap_kib,
        hwm_kib,
        cgroup_current_bytes: cgroup.current_bytes,
        cgroup_swap_bytes: cgroup.swap_bytes,
        cgroup_memory_peak_bytes: cgroup.peak_bytes,
        cgroup_oom: cgroup.oom,
        cgroup_oom_kill: cgroup.oom_kill,
        cgroup_directory: cgroup.directory,
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
    allow_process_exit: bool,
    ready_file: Option<&Path>,
    target_binary_name: Option<&str>,
) -> Result<(), String> {
    const MAX_DURATION_SECS: u64 = 86_400;
    const MAX_INTERVAL_MS: u64 = 60_000;

    if duration_secs == 0 || duration_secs > MAX_DURATION_SECS {
        return Err(format!("duration-secs must be in 1..={MAX_DURATION_SECS}"));
    }
    if interval_ms == 0 || interval_ms > MAX_INTERVAL_MS {
        return Err(format!("interval-ms must be in 1..={MAX_INTERVAL_MS}"));
    }
    if target_binary_name.is_some_and(|name| {
        name.is_empty() || Path::new(name).file_name().and_then(|part| part.to_str()) != Some(name)
    }) {
        return Err("target-binary-name must be a non-empty executable basename".to_string());
    }
    if ready_file.is_some_and(|ready_file| ready_file == output) {
        return Err("ready-file and output must name different files".to_string());
    }
    if let Some(ready_file) = ready_file {
        match fs::remove_file(ready_file) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                return Err(format!(
                    "failed to clear ready file {}: {error}",
                    ready_file.display()
                ));
            }
        }
    }

    #[cfg(target_os = "linux")]
    let expected_start_time = if allow_process_exit {
        let (state, start_time) = process_identity(pid)?
            .ok_or_else(|| format!("process {pid} does not exist before sampling starts"))?;
        if is_terminal_process_state(state) {
            return Err(format!("process {pid} exited before sampling starts"));
        }
        Some(start_time)
    } else {
        None
    };
    #[cfg(not(target_os = "linux"))]
    {
        if allow_process_exit {
            return Err("allow-process-exit is supported only on Linux".to_string());
        }
        if target_binary_name.is_some() {
            return Err("target-binary-name is supported only on Linux".to_string());
        }
    }

    let duration = Duration::from_secs(duration_secs);
    let interval = Duration::from_millis(interval_ms);
    let started = Instant::now();
    let file = fs::File::create(output)
        .map_err(|error| format!("failed to create {}: {error}", output.display()))?;
    let mut writer = BufWriter::new(file);
    let mut warned_cgroup_unavailable = false;
    let mut sample_count = 0usize;
    let mut target_sample_count = 0usize;
    #[cfg(target_os = "linux")]
    let mut last_sample: Option<MemorySample> = None;

    loop {
        #[cfg(target_os = "linux")]
        if let Some(expected_start_time) = expected_start_time
            && target_exited_after_sample(
                pid,
                expected_start_time,
                target_sample_count,
                target_binary_name,
            )?
        {
            if allow_process_exit && let Some(last_sample) = last_sample.as_ref() {
                write_final_cgroup_sample(&mut writer, output, last_sample, started.elapsed())?;
            }
            break;
        }

        let elapsed_ms = u64::try_from(started.elapsed().as_millis())
            .map_err(|error| format!("elapsed time exceeds the sampler range: {error}"))?;
        let sample = match sample_linux(pid, elapsed_ms) {
            Ok(sample) => sample,
            Err(sample_error) => {
                #[cfg(target_os = "linux")]
                if let Some(expected_start_time) = expected_start_time
                    && target_exited_after_sample(
                        pid,
                        expected_start_time,
                        target_sample_count,
                        target_binary_name,
                    )?
                {
                    if allow_process_exit && let Some(last_sample) = last_sample.as_ref() {
                        write_final_cgroup_sample(
                            &mut writer,
                            output,
                            last_sample,
                            started.elapsed(),
                        )?;
                    }
                    break;
                }
                return Err(sample_error);
            }
        };
        if (sample.cgroup_current_bytes.is_none()
            || sample.cgroup_memory_peak_bytes.is_none()
            || sample.cgroup_oom.is_none()
            || sample.cgroup_oom_kill.is_none())
            && !warned_cgroup_unavailable
        {
            eprintln!(
                "sample-memory: complete cgroup v2 memory counters are unavailable; cgroup fields may be null"
            );
            warned_cgroup_unavailable = true;
        }
        write_sample(&mut writer, &sample)?;
        writer
            .flush()
            .map_err(|error| format!("failed to flush {}: {error}", output.display()))?;
        sample_count += 1;
        if target_binary_name.is_none_or(|name| sample.binary_name == name) {
            target_sample_count += 1;
        }
        #[cfg(target_os = "linux")]
        {
            last_sample = Some(sample);
        }
        if sample_count == 1
            && let Some(ready_file) = ready_file
        {
            fs::write(ready_file, "sample-flushed\n").map_err(|error| {
                format!(
                    "failed to signal ready file {}: {error}",
                    ready_file.display()
                )
            })?;
        }

        let elapsed = started.elapsed();
        if elapsed >= duration {
            if allow_process_exit {
                return Err(format!(
                    "process {pid} did not exit within {duration_secs} seconds"
                ));
            }
            break;
        }
        std::thread::sleep(interval.min(duration.saturating_sub(elapsed)));
    }

    if let Some(target_binary_name) = target_binary_name
        && target_sample_count == 0
    {
        return Err(format!(
            "process {pid} produced no sample for target binary {target_binary_name}"
        ));
    }

    Ok(())
}

#[cfg(target_os = "linux")]
fn process_identity(pid: u32) -> Result<Option<(char, u64)>, String> {
    let path = PathBuf::from(format!("/proc/{pid}/stat"));
    let stat = match fs::read_to_string(&path) {
        Ok(stat) => stat,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("failed to read {}: {error}", path.display())),
    };
    let close = stat
        .rfind(')')
        .ok_or_else(|| format!("{} lacks a command terminator", path.display()))?;
    let fields = stat[close + 1..].split_whitespace().collect::<Vec<_>>();
    let state = fields
        .first()
        .and_then(|field| field.chars().next())
        .ok_or_else(|| format!("{} lacks a process state", path.display()))?;
    let start_time = fields
        .get(19)
        .ok_or_else(|| format!("{} lacks a process start time", path.display()))?
        .parse::<u64>()
        .map_err(|error| {
            format!(
                "{} has an invalid process start time: {error}",
                path.display()
            )
        })?;
    Ok(Some((state, start_time)))
}

#[cfg(target_os = "linux")]
fn target_exited_after_sample(
    pid: u32,
    expected_start_time: u64,
    sample_count: usize,
    target_binary_name: Option<&str>,
) -> Result<bool, String> {
    let no_sample_error = || match target_binary_name {
        Some(name) => format!("process {pid} exited before a sample of target binary {name}"),
        None => format!("process {pid} exited before its first memory sample"),
    };
    let Some((state, observed_start_time)) = process_identity(pid)? else {
        return if sample_count > 0 {
            Ok(true)
        } else {
            Err(no_sample_error())
        };
    };
    if observed_start_time != expected_start_time {
        return Err(format!(
            "process ID {pid} was reused during sampling (start time {expected_start_time} became {observed_start_time})"
        ));
    }
    if is_terminal_process_state(state) {
        return if sample_count > 0 {
            Ok(true)
        } else {
            Err(no_sample_error())
        };
    }
    Ok(false)
}

#[cfg(target_os = "linux")]
fn is_terminal_process_state(state: char) -> bool {
    matches!(state, 'Z' | 'X')
}

fn write_sample(writer: &mut impl Write, sample: &MemorySample) -> Result<(), String> {
    fn optional_number(value: Option<u64>) -> String {
        value.map_or_else(|| "null".to_string(), |number| number.to_string())
    }

    let process_memory = sample.process_alive.then_some(());

    writeln!(
        writer,
        "{{\"binary_name\":{},\"process_alive\":{},\"elapsed_ms\":{},\"rss_kib\":{},\"swap_kib\":{},\"hwm_kib\":{},\"cgroup_current_bytes\":{},\"cgroup_swap_bytes\":{},\"cgroup_memory_peak_bytes\":{},\"cgroup_oom\":{},\"cgroup_oom_kill\":{}}}",
        serde_json::to_string(&sample.binary_name)
            .map_err(|error| format!("failed to serialize executable name: {error}"))?,
        sample.process_alive,
        sample.elapsed_ms,
        process_memory.map_or_else(|| "null".to_string(), |_| sample.rss_kib.to_string()),
        process_memory.map_or_else(|| "null".to_string(), |_| sample.swap_kib.to_string()),
        process_memory.map_or_else(|| "null".to_string(), |_| sample.hwm_kib.to_string()),
        optional_number(sample.cgroup_current_bytes),
        optional_number(sample.cgroup_swap_bytes),
        optional_number(sample.cgroup_memory_peak_bytes),
        optional_number(sample.cgroup_oom),
        optional_number(sample.cgroup_oom_kill)
    )
    .map_err(|error| format!("failed to write memory sample: {error}"))
}

#[cfg(target_os = "linux")]
fn cgroup_memory(pid: u32) -> Result<CgroupMemory, String> {
    let membership_path = PathBuf::from(format!("/proc/{pid}/cgroup"));
    let membership = fs::read_to_string(&membership_path)
        .map_err(|error| format!("failed to read {}: {error}", membership_path.display()))?;
    let Some(group_path) = membership.lines().find_map(|line| {
        line.strip_prefix("0::")
            .map(|path| PathBuf::from(path.trim()))
    }) else {
        return Ok(CgroupMemory::default());
    };

    let mountinfo = fs::read_to_string("/proc/self/mountinfo")
        .map_err(|error| format!("failed to read /proc/self/mountinfo: {error}"))?;
    let Some(group_directory) = cgroup_mount_directory(&mountinfo, &group_path) else {
        return Ok(CgroupMemory::default());
    };
    cgroup_memory_at(&group_directory)
}

#[cfg(target_os = "linux")]
fn cgroup_memory_at(group_directory: &Path) -> Result<CgroupMemory, String> {
    let current = read_optional_counter(&group_directory.join("memory.current"))?;
    let swap = read_optional_counter(&group_directory.join("memory.swap.current"))?;
    let peak = read_optional_counter(&group_directory.join("memory.peak"))?;
    let events = read_optional_file(&group_directory.join("memory.events"))?
        .map(|contents| parse_cgroup_events(&contents))
        .transpose()?;
    Ok(CgroupMemory {
        current_bytes: current,
        swap_bytes: swap,
        peak_bytes: peak,
        oom: events.map(|(oom, _)| oom),
        oom_kill: events.map(|(_, oom_kill)| oom_kill),
        directory: Some(group_directory.to_path_buf()),
    })
}

#[cfg(target_os = "linux")]
fn write_final_cgroup_sample(
    writer: &mut impl Write,
    output: &Path,
    last_sample: &MemorySample,
    elapsed: Duration,
) -> Result<(), String> {
    let Some(group_directory) = last_sample.cgroup_directory.as_deref() else {
        return Ok(());
    };
    let cgroup = cgroup_memory_at(group_directory)?;
    let elapsed_ms = u64::try_from(elapsed.as_millis())
        .map_err(|error| format!("elapsed time exceeds the sampler range: {error}"))?
        .max(last_sample.elapsed_ms.saturating_add(1));
    let sample = MemorySample {
        binary_name: last_sample.binary_name.clone(),
        process_alive: false,
        elapsed_ms,
        rss_kib: 0,
        swap_kib: 0,
        hwm_kib: 0,
        cgroup_current_bytes: cgroup.current_bytes,
        cgroup_swap_bytes: cgroup.swap_bytes,
        cgroup_memory_peak_bytes: cgroup.peak_bytes,
        cgroup_oom: cgroup.oom,
        cgroup_oom_kill: cgroup.oom_kill,
        cgroup_directory: cgroup.directory,
    };
    write_sample(writer, &sample)?;
    writer
        .flush()
        .map_err(|error| format!("failed to flush {}: {error}", output.display()))
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

#[cfg(target_os = "linux")]
fn read_optional_file(path: &Path) -> Result<Option<String>, String> {
    match fs::read_to_string(path) {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(format!("failed to read {}: {error}", path.display())),
    }
}

#[cfg(target_os = "linux")]
fn parse_cgroup_events(input: &str) -> Result<(u64, u64), String> {
    let mut oom = None;
    let mut oom_kill = None;
    for line in input.lines() {
        let mut fields = line.split_whitespace();
        let Some(name) = fields.next() else {
            continue;
        };
        let value = fields
            .next()
            .ok_or_else(|| format!("cgroup event {name} has no counter"))?
            .parse::<u64>()
            .map_err(|error| format!("cgroup event {name} has an invalid counter: {error}"))?;
        if fields.next().is_some() {
            return Err(format!("cgroup event {name} has extra fields"));
        }
        let slot = match name {
            "oom" => &mut oom,
            "oom_kill" => &mut oom_kill,
            _ => continue,
        };
        if slot.replace(value).is_some() {
            return Err(format!("cgroup event {name} is duplicated"));
        }
    }
    let oom = oom.ok_or_else(|| "cgroup event oom is missing".to_string())?;
    let oom_kill = oom_kill.ok_or_else(|| "cgroup event oom_kill is missing".to_string())?;
    Ok((oom, oom_kill))
}

#[cfg(test)]
mod tests {
    use super::{parse_status, run};
    #[cfg(target_os = "linux")]
    use std::process::Command;

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

        assert!(run(std::process::id(), 1, 0, &output, false, None, None).is_err());
        assert!(!output.exists());
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn sampler_can_finish_after_the_target_process_exits() {
        let directory = tempfile::tempdir().expect("temporary directory should be available");
        let output = directory.path().join("samples.jsonl");
        let mut child = Command::new("sleep")
            .arg("1")
            .spawn()
            .expect("short-lived target should start");
        let pid = child.id();
        let waiter = std::thread::spawn(move || child.wait());

        let result = run(pid, 30, 50, &output, true, None, None);

        assert!(result.is_ok(), "expected clean target exit, got {result:?}");
        assert!(
            waiter
                .join()
                .expect("target wait thread should join")
                .unwrap()
                .success(),
            "target should exit successfully"
        );
        let samples = std::fs::read_to_string(output)
            .expect("successful sampling should preserve its sample file");
        assert!(
            samples.lines().count() >= 2,
            "target exit must not be accepted before multiple samples"
        );
    }
}
