#![cfg(target_os = "linux")]

use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

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
        assert_eq!(sample["binary_name"].as_str(), Some("sleep"));
        assert_eq!(sample["process_alive"].as_bool(), Some(true));
        assert!(sample["elapsed_ms"].is_u64());
        assert!(sample["rss_kib"].is_u64());
        assert!(sample["swap_kib"].is_u64());
        assert!(sample["hwm_kib"].is_u64());
        assert!(sample.get("cgroup_current_bytes").is_some());
        assert!(sample.get("cgroup_swap_bytes").is_some());
        assert!(sample.get("cgroup_memory_peak_bytes").is_some());
        assert!(sample.get("cgroup_oom").is_some());
        assert!(sample.get("cgroup_oom_kill").is_some());
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

#[test]
fn sampler_ready_file_signals_a_flushed_sample_before_target_release() {
    let mut process = SleepProcess::start();
    let directory = tempfile::tempdir().expect("temporary directory should be available");
    let output = directory.path().join("samples.jsonl");
    let ready = directory.path().join("sampler.ready");
    let mut sampler = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args([
            "sample-memory",
            "--pid",
            &process.pid().to_string(),
            "--duration-secs",
            "30",
            "--interval-ms",
            "50",
            "--allow-process-exit",
            "--output",
        ])
        .arg(&output)
        .arg("--ready-file")
        .arg(&ready)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("sampler command should start");

    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() && Instant::now() < deadline {
        assert!(
            sampler
                .try_wait()
                .expect("sampler status should be readable")
                .is_none(),
            "sampler exited before signaling its first durable sample"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(ready.exists(), "sampler did not signal its first sample");
    let samples =
        std::fs::read_to_string(&output).expect("ready signal must follow a flushed sample file");
    assert_eq!(samples.lines().count(), 1);

    process.stop();
    let result = sampler
        .wait_with_output()
        .expect("sampler should exit after the target exits");
    assert!(
        result.status.success(),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
    let contents = std::fs::read_to_string(&output).expect("final cgroup sample should exist");
    let samples: Vec<Value> = contents
        .lines()
        .map(|line| serde_json::from_str(line).expect("each line should be JSON"))
        .collect();
    let final_sample = samples
        .last()
        .expect("sampler should emit a final cgroup snapshot");
    assert_eq!(final_sample["binary_name"].as_str(), Some("sleep"));
    assert_eq!(final_sample["process_alive"].as_bool(), Some(false));
    assert!(final_sample["rss_kib"].is_null());
    assert!(final_sample["hwm_kib"].is_null());
    assert!(final_sample["cgroup_oom"].is_u64() || final_sample["cgroup_oom"].is_null());
}

#[test]
fn sampler_rejects_wrapper_only_samples_after_target_exit() {
    let directory = tempfile::tempdir().expect("temporary directory should be available");
    let wrapper_source = directory.path().join("memory_exec_barrier.rs");
    let wrapper = directory.path().join("memory_exec_barrier");
    std::fs::write(
        &wrapper_source,
        include_str!("../../memory-mcp/tests/fixtures/memory_exec_barrier.rs"),
    )
    .expect("wrapper source should be copied into the isolated fixture");
    let compile = Command::new("rustc")
        .arg("--edition=2024")
        .arg(&wrapper_source)
        .arg("-o")
        .arg(&wrapper)
        .output()
        .expect("pinned rustc should compile the std-only wrapper");
    assert!(
        compile.status.success(),
        "wrapper compile failed: {}",
        String::from_utf8_lossy(&compile.stderr)
    );

    let gate = directory.path().join("release.gate");
    let output = directory.path().join("samples.jsonl");
    let ready = directory.path().join("sampler.ready");
    let mut target = Command::new(&wrapper)
        .args(["--gate", gate.to_str().expect("UTF-8 gate path")])
        .arg("--deadline-secs")
        .arg("5")
        .arg("--")
        .arg("/bin/sleep")
        .arg("1")
        .spawn()
        .expect("exec-barrier process should start");
    let mut sampler = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args([
            "sample-memory",
            "--pid",
            &target.id().to_string(),
            "--duration-secs",
            "30",
            "--interval-ms",
            "10",
            "--allow-process-exit",
            "--target-binary-name",
            "memory_mcp_http",
            "--output",
        ])
        .arg(&output)
        .arg("--ready-file")
        .arg(&ready)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("sampler command should start");

    let deadline = Instant::now() + Duration::from_secs(5);
    while !ready.exists() && Instant::now() < deadline {
        assert!(
            sampler
                .try_wait()
                .expect("sampler status should be readable")
                .is_none(),
            "sampler exited before signaling its first durable sample"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(ready.exists(), "sampler did not signal its first sample");
    std::fs::write(&gate, "release\n").expect("target gate should open");
    let result = sampler
        .wait_with_output()
        .expect("sampler should finish after the target exits");
    let _ = target.wait();

    assert!(!result.status.success());
    assert!(
        String::from_utf8_lossy(&result.stderr).contains("target binary"),
        "{}",
        String::from_utf8_lossy(&result.stderr)
    );
}
