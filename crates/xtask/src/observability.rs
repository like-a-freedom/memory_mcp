//! Runs the observability checkers, so they stop drifting from the code.
//!
//! `observability/` holds 40 recording rules, 18 alerts, 2 dashboards and 4
//! Python checkers. Nothing in the repository reached any of them: no
//! workflow, no Makefile target, no cargo target. The checkers read local
//! files and need no running Prometheus, so the only thing standing between
//! them and CI was that nobody called them.
//!
//! They are Python, and ADR-0064 requires CI to run nothing outside cargo.
//! That is not a reason to leave them unwired — it is the reason the entry
//! point is a cargo subcommand. `cargo run -p xtask -- check-observability`
//! is a cargo invocation, so a workflow that runs cargo runs this too.

use std::path::Path;
use std::process::{Child, Command, ExitStatus, Stdio};
use std::time::{Duration, Instant};

use crate::pack::PackError;

const SCRIPT_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The four scripts, in the order they must run.
///
/// `build_dashboards` goes first because it regenerates the committed JSON and
/// the other three read it. A checker run against a stale dashboard proves
/// nothing about the dashboard that ships.
const SCRIPTS: &[&str] = &[
    "build_dashboards.py",
    "check_rules.py",
    "check_alerts.py",
    "check_dashboards.py",
];

/// Run every checker in `scripts_dir`.
///
/// The order is `SCRIPTS` and the first failure stops the run, because the
/// later checkers read what the earlier ones produce.
pub fn run(scripts_dir: &Path) -> Result<(), PackError> {
    for script in SCRIPTS {
        run_one(scripts_dir, script)?;
    }
    Ok(())
}

fn run_one(scripts_dir: &Path, script: &str) -> Result<(), PackError> {
    let path = scripts_dir.join(script);
    if !path.is_file() {
        return Err(PackError::Observability(format!(
            "{} is missing. The observability checkers are part of the \
             repository: a missing one means the set is incomplete, not that \
             there is nothing to check.",
            path.display()
        )));
    }

    // Regular files cannot keep a reader blocked when a checker descendant
    // inherits stdout/stderr. They also avoid pipe-capacity deadlocks.
    let stdout = tempfile::NamedTempFile::new().map_err(|error| {
        PackError::Observability(format!("could not capture {script} stdout: {error}"))
    })?;
    let stderr = tempfile::NamedTempFile::new().map_err(|error| {
        PackError::Observability(format!("could not capture {script} stderr: {error}"))
    })?;
    let stdout_writer = stdout.reopen().map_err(|error| {
        PackError::Observability(format!("could not clone {script} stdout: {error}"))
    })?;
    let stderr_writer = stderr.reopen().map_err(|error| {
        PackError::Observability(format!("could not clone {script} stderr: {error}"))
    })?;
    let mut child = Command::new("python3")
        .arg(&path)
        .stdin(Stdio::null())
        .stdout(Stdio::from(stdout_writer))
        .stderr(Stdio::from(stderr_writer))
        .spawn()
        .map_err(|e| {
            PackError::Observability(format!("could not run python3 on {}: {e}", path.display()))
        })?;
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < SCRIPT_TIMEOUT => {
                std::thread::sleep(POLL_INTERVAL);
            }
            Ok(None) => {
                kill_and_reap(&mut child)?;
                break None;
            }
            Err(error) => {
                kill_and_reap(&mut child)?;
                return Err(PackError::Observability(format!(
                    "could not wait for {script}: {error}"
                )));
            }
        }
    };
    // Independent read offsets cannot seek a descendant's inherited writer.
    let stdout_file = stdout.reopen().map_err(|error| {
        PackError::Observability(format!("could not reopen {script} stdout: {error}"))
    })?;
    let stderr_file = stderr.reopen().map_err(|error| {
        PackError::Observability(format!("could not reopen {script} stderr: {error}"))
    })?;
    let stdout = read_child_output(stdout_file, script)?;
    let stderr = read_child_output(stderr_file, script)?;
    let Some(status) = status else {
        return Err(PackError::Observability(format!(
            "{script} timed out after {} seconds",
            SCRIPT_TIMEOUT.as_secs()
        )));
    };

    if status.success() {
        let stdout = String::from_utf8_lossy(&stdout);
        let summary = stdout.lines().last().unwrap_or("").trim();
        if !summary.is_empty() {
            println!("{script}: {summary}");
        }
        return Ok(());
    }

    Err(PackError::Observability(format!(
        "{script} failed:\n{}{}",
        String::from_utf8_lossy(&stdout).trim_end(),
        indent(&String::from_utf8_lossy(&stderr)),
    )))
}

fn read_child_output(mut file: std::fs::File, script: &str) -> Result<Vec<u8>, PackError> {
    use std::io::{Read, Seek};
    let mut output = Vec::new();
    // Read only the snapshot present when the immediate checker exits, even
    // if a descendant continues writing to the inherited file descriptor.
    let length = file
        .metadata()
        .map_err(|error| {
            PackError::Observability(format!("could not inspect {script} output: {error}"))
        })?
        .len();
    file.rewind().map_err(|error| {
        PackError::Observability(format!("could not rewind {script} output: {error}"))
    })?;
    file.take(length)
        .read_to_end(&mut output)
        .map_err(|error| {
            PackError::Observability(format!("could not read {script} output: {error}"))
        })?;
    Ok(output)
}

fn kill_and_reap(child: &mut Child) -> Result<ExitStatus, PackError> {
    let _ = child.kill();
    child.wait().map_err(|error| {
        PackError::Observability(format!("could not reap timed-out checker: {error}"))
    })
}

fn indent(text: &str) -> String {
    if text.trim().is_empty() {
        return String::new();
    }
    let mut out = String::from("\n--- stderr ---\n");
    for line in text.lines() {
        out.push_str(line);
        out.push('\n');
    }
    out
}

/// The `observability/` directory of the workspace this xtask belongs to.
pub fn scripts_dir() -> std::path::PathBuf {
    // CARGO_MANIFEST_DIR is crates/xtask; observability/ is at the root.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../observability")
        .canonicalize()
        .unwrap_or_else(|_| Path::new(env!("CARGO_MANIFEST_DIR")).join("../../observability"))
}

#[cfg(test)]
mod tests {
    //! Tooling integration tests for the public runner and external checker
    //! processes; these are not product-behavior unit coverage.

    use super::*;

    fn write_script(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).expect("write script");
    }

    fn complete_scripts(dir: &Path) {
        write_script(dir, SCRIPTS[0], "print('dashboards built')\n");
        write_script(dir, SCRIPTS[1], "print('rules ok')\n");
        write_script(dir, SCRIPTS[2], "print('alerts ok')\n");
        write_script(dir, SCRIPTS[3], "print('dashboards ok')\n");
    }

    #[test]
    fn run_reports_the_first_failing_script() {
        let dir = tempfile::tempdir().expect("temp dir");
        write_script(
            dir.path(),
            "build_dashboards.py",
            "print('dashboards built')\n",
        );
        write_script(
            dir.path(),
            "check_rules.py",
            "import sys\nprint('rule 3 reads a metric the crate does not export')\nsys.exit(1)\n",
        );
        write_script(
            dir.path(),
            "check_alerts.py",
            "from pathlib import Path\n(Path(__file__).parent / 'later-ran').write_text('alerts')\n",
        );
        write_script(
            dir.path(),
            "check_dashboards.py",
            "from pathlib import Path\n(Path(__file__).parent / 'later-ran').write_text('dashboards')\n",
        );

        let error = run(dir.path())
            .expect_err("a failing checker must be reported")
            .to_string();
        assert!(
            error.contains("check_rules.py"),
            "the error must name the failing script, got: {error}"
        );
        assert!(
            error.contains("does not export"),
            "the checker's own message must survive, got: {error}"
        );
        assert!(
            !dir.path().join("later-ran").exists(),
            "a later checker must not produce its observable marker"
        );
    }

    #[test]
    fn run_passes_when_every_checker_succeeds() {
        let dir = tempfile::tempdir().expect("temp dir");
        complete_scripts(dir.path());
        assert!(run(dir.path()).is_ok());
    }

    #[test]
    fn a_missing_checker_is_reported_rather_than_skipped() {
        let dir = tempfile::tempdir().expect("temp dir");
        write_script(dir.path(), "check_rules.py", "print('rules ok')\n");
        write_script(dir.path(), "check_alerts.py", "print('alerts ok')\n");
        write_script(
            dir.path(),
            "check_dashboards.py",
            "print('dashboards ok')\n",
        );
        let error = run(dir.path())
            .expect_err("a missing checker is not a pass")
            .to_string();
        assert!(
            error.contains("build_dashboards.py") && error.contains("missing"),
            "got: {error}"
        );
    }

    #[test]
    fn a_checker_that_exceeds_its_deadline_is_killed_and_reaped() {
        let dir = tempfile::tempdir().expect("temp dir");
        write_script(
            dir.path(),
            "build_dashboards.py",
            "import os, time\nfrom pathlib import Path\n(Path(__file__).parent / 'started').write_text(str(os.getpid()))\ntime.sleep(60)\n",
        );
        let started = Instant::now();

        let error = run(dir.path())
            .expect_err("a checker that never exits must time out")
            .to_string();

        assert!(dir.path().join("started").exists(), "the child did start");
        assert!(error.contains("timed out after 10 seconds"), "got: {error}");
        #[cfg(unix)]
        {
            let pid = std::fs::read_to_string(dir.path().join("started")).expect("checker PID");
            let probe = Command::new("/bin/kill")
                .args(["-0", pid.trim()])
                .stderr(Stdio::null())
                .status()
                .expect("probe checker lifetime");
            assert!(!probe.success(), "timed-out checker must be reaped");
        }
        assert!(
            started.elapsed() < Duration::from_secs(15),
            "the checker exceeded its bounded lifetime: {:?}",
            started.elapsed()
        );
    }

    #[test]
    fn inherited_output_does_not_hold_the_runner_open() {
        let dir = tempfile::tempdir().expect("temp dir");
        complete_scripts(dir.path());
        write_script(
            dir.path(),
            "build_dashboards.py",
            r#"import subprocess, sys
from pathlib import Path
root = Path(__file__).parent
code = '''import sys, time
from pathlib import Path
root = Path(sys.argv[1])
deadline = time.monotonic() + 15
while not (root / "release").exists() and time.monotonic() < deadline:
    time.sleep(0.01)
(root / "finished").write_text("yes")
'''
subprocess.Popen([sys.executable, "-c", code, str(root)])
print("checker complete")
"#,
        );
        let path = dir.path().to_owned();
        let (send, receive) = std::sync::mpsc::channel();
        let runner = std::thread::spawn(move || {
            let _ = send.send(run(&path));
        });
        let result = receive.recv_timeout(Duration::from_secs(5));
        // Release the fixture descendant even when the regression is present.
        std::fs::write(dir.path().join("release"), "yes").expect("release descendant");
        let cleanup_deadline = Instant::now() + Duration::from_secs(5);
        while !dir.path().join("finished").exists() && Instant::now() < cleanup_deadline {
            std::thread::sleep(POLL_INTERVAL);
        }
        assert!(
            dir.path().join("finished").exists(),
            "fixture descendant finished"
        );
        runner
            .join()
            .expect("runner joined after descendant release");
        assert!(
            result
                .expect("runner must not wait for descendant output")
                .is_ok()
        );
    }
}
