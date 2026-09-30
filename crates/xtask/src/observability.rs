//! Runs the observability checkers, so they stop drifting from the code.
//!
//! `observability/` holds 31 recording rules, 15 alerts, 4 dashboards and 4
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
use std::process::Command;

use crate::pack::PackError;

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

    let output = Command::new("python3").arg(&path).output().map_err(|e| {
        PackError::Observability(format!("could not run python3 on {}: {e}", path.display()))
    })?;

    if output.status.success() {
        let stdout = String::from_utf8_lossy(&output.stdout);
        let summary = stdout.lines().last().unwrap_or("").trim();
        if !summary.is_empty() {
            println!("{script}: {summary}");
        }
        return Ok(());
    }

    Err(PackError::Observability(format!(
        "{script} failed:\n{}{}",
        String::from_utf8_lossy(&output.stdout).trim_end(),
        indent(&String::from_utf8_lossy(&output.stderr)),
    )))
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
    use super::*;

    fn temp_dir(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("xtask-observability-{name}"));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn write_script(dir: &Path, name: &str, body: &str) {
        std::fs::write(dir.join(name), body).expect("write script");
    }

    #[test]
    fn run_reports_the_first_failing_script() {
        let dir = temp_dir("first-failure");
        write_script(&dir, "build_dashboards.py", "print('dashboards built')\n");
        write_script(
            &dir,
            "check_rules.py",
            "import sys\nprint('rule 3 reads a metric the crate does not export')\nsys.exit(1)\n",
        );
        // The later scripts are correct and must not be reached: if they ran,
        // the error would name one of them instead.
        write_script(&dir, "check_alerts.py", "print('alerts ok')\n");
        write_script(&dir, "check_dashboards.py", "print('dashboards ok')\n");

        let error = run(&dir)
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
            !error.contains("check_alerts.py"),
            "the run must stop at the first failure, got: {error}"
        );
    }

    #[test]
    fn run_passes_when_every_checker_succeeds() {
        let dir = temp_dir("all-pass");
        for script in SCRIPTS {
            write_script(&dir, script, "print('ok')\n");
        }
        assert!(run(&dir).is_ok());
    }

    #[test]
    fn a_missing_checker_is_reported_rather_than_skipped() {
        let dir = temp_dir("missing");
        for script in SCRIPTS.iter().skip(1) {
            write_script(&dir, script, "print('ok')\n");
        }
        let error = run(&dir)
            .expect_err("a missing checker is not a pass")
            .to_string();
        assert!(
            error.contains("build_dashboards.py") && error.contains("missing"),
            "got: {error}"
        );
    }
}
