#![cfg(target_os = "linux")]

use std::process::{Child, Command};

use serde_json::Value;

struct SleepProcess(Child);

impl SleepProcess {
    fn start() -> Self {
        let child = Command::new("/bin/sleep")
            .arg("10")
            .spawn()
            .expect("Linux sleep process should start");
        Self(child)
    }

    fn pid(&self) -> u32 {
        self.0.id()
    }

    fn stop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

impl Drop for SleepProcess {
    fn drop(&mut self) {
        self.stop();
    }
}

#[test]
fn sampler_writes_json_lines_for_a_live_child() {
    let process = SleepProcess::start();
    let directory = tempfile::tempdir().expect("temporary directory should be available");
    let output = directory.path().join("samples.jsonl");
    let result = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args([
            "sample-memory",
            "--pid",
            &process.pid().to_string(),
            "--duration-secs",
            "1",
            "--interval-ms",
            "250",
            "--output",
        ])
        .arg(&output)
        .output()
        .expect("sampler command should run");

    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let contents = std::fs::read_to_string(output).expect("sampler output should exist");
    let samples: Vec<Value> = contents
        .lines()
        .map(|line| serde_json::from_str(line).expect("each line should be JSON"))
        .collect();

    assert!(samples.len() >= 2);
    assert!(
        samples
            .windows(2)
            .all(|pair| { pair[0]["elapsed_ms"].as_u64() < pair[1]["elapsed_ms"].as_u64() })
    );
    for sample in samples {
        assert!(sample["elapsed_ms"].is_u64());
        assert!(sample["rss_kib"].is_u64());
        assert!(sample["swap_kib"].is_u64());
        assert!(sample["hwm_kib"].is_u64());
        assert!(sample.get("cgroup_current_bytes").is_some());
        assert!(sample.get("cgroup_swap_bytes").is_some());
        assert_eq!(
            sample["cgroup_current_bytes"].is_null(),
            sample["cgroup_swap_bytes"].is_null()
        );
    }
}

#[test]
fn sampler_fails_for_a_child_that_exited() {
    let mut process = SleepProcess::start();
    let pid = process.pid();
    process.stop();
    let directory = tempfile::tempdir().expect("temporary directory should be available");
    let output = directory.path().join("samples.jsonl");
    let result = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args([
            "sample-memory",
            "--pid",
            &pid.to_string(),
            "--duration-secs",
            "1",
            "--interval-ms",
            "250",
            "--output",
        ])
        .arg(output)
        .output()
        .expect("sampler command should run");

    assert!(!result.status.success());
    assert!(String::from_utf8_lossy(&result.stderr).contains("failed to read /proc/"));
}
