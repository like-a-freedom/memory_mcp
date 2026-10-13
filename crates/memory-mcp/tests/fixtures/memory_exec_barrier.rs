//! Standalone std-only process wrapper used by the Linux memory harness.

use std::ffi::OsString;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::process::Command;
use std::thread;
use std::time::{Duration, Instant};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

fn main() {
    let (gate, deadline_secs, program, command_args) = match parse_args() {
        Ok(parsed) => parsed,
        Err(error) => {
            eprintln!("memory_exec_barrier: {error}");
            std::process::exit(2);
        }
    };
    let pid = std::process::id();
    let start_time_ticks = match process_start_time_ticks(pid) {
        Ok(value) => value,
        Err(error) => {
            eprintln!("memory_exec_barrier: cannot read process start time: {error}");
            std::process::exit(2);
        }
    };
    println!(
        "MEMORY_EXEC_BARRIER_WAITING pid={pid} start_time_ticks={start_time_ticks}"
    );
    let _ = std::io::stdout().flush();

    let started = Instant::now();
    let deadline = Duration::from_secs(deadline_secs);
    loop {
        match fs::read_to_string(&gate) {
            Ok(contents) if contents == "release\n" => break,
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => {
                eprintln!("memory_exec_barrier: cannot read gate: {error}");
                std::process::exit(2);
            }
        }
        if started.elapsed() >= deadline {
            eprintln!("memory_exec_barrier: release gate deadline expired");
            std::process::exit(124);
        }
        thread::park_timeout(Duration::from_millis(10));
    }

    #[cfg(unix)]
    {
        let error = Command::new(program)
            .args(&command_args)
            .env("MEMORY_EXEC_BARRIER_PID", pid.to_string())
            .env(
                "MEMORY_EXEC_BARRIER_START_TICKS",
                start_time_ticks.to_string(),
            )
            .exec();
        eprintln!("memory_exec_barrier: exec failed: {error}");
        std::process::exit(126);
    }

    #[cfg(not(unix))]
    {
        eprintln!("memory_exec_barrier: Unix exec is required");
        std::process::exit(2);
    }
}

fn parse_args() -> Result<(PathBuf, u64, OsString, Vec<OsString>), String> {
    let mut args = std::env::args_os().skip(1);
    let mut gate = None;
    let mut deadline_secs = None;
    loop {
        let Some(argument) = args.next() else {
            return Err("expected --gate PATH --deadline-secs N -- PROGRAM [ARGS...]".into());
        };
        if argument == "--" {
            break;
        }
        if argument == "--gate" {
            gate = Some(PathBuf::from(
                args.next().ok_or("--gate requires a path")?,
            ));
        } else if argument == "--deadline-secs" {
            let raw = args.next().ok_or("--deadline-secs requires a value")?;
            let seconds = raw
                .to_str()
                .ok_or("deadline must be UTF-8")?
                .parse::<u64>()
                .map_err(|error| format!("invalid deadline: {error}"))?;
            if seconds == 0 || seconds > 86_400 {
                return Err("deadline must be in 1..=86400 seconds".into());
            }
            deadline_secs = Some(seconds);
        } else {
            return Err(format!("unknown option {argument:?}"));
        }
    }
    let program = args.next().ok_or("missing command after --")?;
    let command_args = args.collect::<Vec<_>>();
    Ok((
        gate.ok_or("--gate is required")?,
        deadline_secs.ok_or("--deadline-secs is required")?,
        program,
        command_args,
    ))
}

#[cfg(target_os = "linux")]
fn process_start_time_ticks(pid: u32) -> std::io::Result<u64> {
    let stat = fs::read_to_string(format!("/proc/{pid}/stat"))?;
    let close = stat.rfind(')').ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "process stat lacks comm terminator",
        )
    })?;
    stat[close + 1..]
        .split_whitespace()
        .nth(19)
        .ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, "process start time missing")
        })?
        .parse()
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))
}

#[cfg(not(target_os = "linux"))]
fn process_start_time_ticks(_pid: u32) -> std::io::Result<u64> {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| u64::try_from(duration.as_micros()).unwrap_or(u64::MAX))
        .map_err(std::io::Error::other)
}
