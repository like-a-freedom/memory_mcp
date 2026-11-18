#![cfg(feature = "streamable-http")]

use std::collections::BTreeMap;
use std::fs;
use std::io::{self, BufRead};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use memory_mcp::config::SurrealConfigBuilder;
use memory_mcp::storage::{DbClient, SurrealDbClient};
use sha2::{Digest, Sha256};
use tempfile::TempDir;

const PRIMARY_BINARY: &str = env!("CARGO_BIN_EXE_memory_mcp");
const HTTP_BINARY: &str = env!("CARGO_BIN_EXE_memory_mcp_http");
const FIXTURE_BINARY_ENV: &str = "SURREALDB3_FIXTURE_BIN";
const PROCESS_TIMEOUT: Duration = Duration::from_secs(90);
const NAMESPACE: &str = "probe";
const DATABASE: &str = "memory";

#[derive(Clone, Copy)]
enum LegacyEngine {
    RocksDb,
    SurrealKv,
}

impl LegacyEngine {
    fn as_str(self) -> &'static str {
        match self {
            Self::RocksDb => "rocksdb",
            Self::SurrealKv => "surrealkv",
        }
    }
}

fn fixture_binary() -> PathBuf {
    std::env::var_os(FIXTURE_BINARY_ENV)
        .map(PathBuf::from)
        .expect("run through tests/support/run-surrealdb4-upgrade-tests.sh")
}

fn seed_fixture(engine: LegacyEngine, path: &Path) {
    let mut command = Command::new(fixture_binary());
    command.arg(engine.as_str()).arg("seed").arg(path);
    let output = run_bounded(command, PROCESS_TIMEOUT);
    assert_success(output, "seed legacy fixture");
}

fn verify_fixture(engine: LegacyEngine, path: &Path) {
    let mut command = Command::new(fixture_binary());
    command.arg(engine.as_str()).arg("verify").arg(path);
    assert_success(
        run_bounded(command, PROCESS_TIMEOUT),
        "verify legacy fixture",
    );
}

fn copy_tree(source: &Path, destination: &Path) -> io::Result<()> {
    fs::create_dir_all(destination)?;
    for entry in fs::read_dir(source)? {
        let entry = entry?;
        let source_path = entry.path();
        let destination_path = destination.join(entry.file_name());
        if entry.file_type()?.is_dir() {
            copy_tree(&source_path, &destination_path)?;
        } else {
            fs::copy(source_path, destination_path)?;
        }
    }
    Ok(())
}

async fn assert_primary_record(path: &Path, allow_upgrade: bool) {
    let mut config = SurrealConfigBuilder::new()
        .db_name(DATABASE)
        .namespace(NAMESPACE)
        .credentials("root", "root")
        .embedded(true)
        .data_dir(path.to_string_lossy())
        .build()
        .expect("valid isolated embedded configuration");
    config.upgrade_from_3x = allow_upgrade;

    let client = SurrealDbClient::connect(&config)
        .await
        .expect("application storage constructor opens the datastore");
    let value = client
        .query(
            "RETURN (SELECT VALUE value FROM known:1)[0]",
            None,
            NAMESPACE,
        )
        .await
        .expect("read known fixture record through application storage adapter");
    assert!(
        value.to_string().contains("surrealdb-3x-data-survives"),
        "application did not read the known record: {value}"
    );
}

fn hash_tree(root: &Path) -> io::Result<BTreeMap<PathBuf, String>> {
    fn visit(
        root: &Path,
        directory: &Path,
        hashes: &mut BTreeMap<PathBuf, String>,
    ) -> io::Result<()> {
        for entry in fs::read_dir(directory)? {
            let entry = entry?;
            let path = entry.path();
            let metadata = entry.metadata()?;
            if metadata.is_dir() {
                visit(root, &path, hashes)?;
            } else if metadata.is_file() {
                let relative = path
                    .strip_prefix(root)
                    .expect("walked path remains beneath root")
                    .to_path_buf();
                let digest = Sha256::digest(fs::read(path)?);
                hashes.insert(relative, hex::encode(digest));
            }
        }
        Ok(())
    }

    let mut hashes = BTreeMap::new();
    visit(root, root, &mut hashes)?;
    Ok(hashes)
}

fn primary_command(path: &Path, allow_upgrade: bool) -> Command {
    let mut command = Command::new(PRIMARY_BINARY);
    command
        .arg("surrealdb-v4-maintenance")
        .arg("--role")
        .arg("primary")
        .env_clear()
        .env("SURREALDB_DATA_DIR", path)
        .env("SURREALDB_DB_NAME", DATABASE)
        .env("SURREALDB_NAMESPACE", NAMESPACE)
        .env("SURREALDB_EMBEDDED", "true")
        .env(
            "SURREAL_EXPERIMENTAL_UPGRADE_FROM_3X",
            allow_upgrade.to_string(),
        )
        .env("RUST_LOG", "info");
    command
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

fn hold_legacy_store(engine: LegacyEngine, path: &Path, ready_file: &Path) -> ChildGuard {
    let mut command = Command::new(fixture_binary());
    let mut child = command
        .arg(engine.as_str())
        .arg("hold")
        .arg(path)
        .env("SURREALDB3_FIXTURE_READY_FILE", ready_file)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .expect("start legacy store lock holder");
    let deadline = Instant::now() + Duration::from_secs(15);

    loop {
        if ready_file.exists() {
            return ChildGuard(child);
        }
        if let Some(status) = child.try_wait().expect("check fixture lock holder") {
            panic!("legacy store lock holder exited before acquiring the lock: {status}");
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let status = child.wait().expect("reap timed-out fixture lock holder");
            panic!("legacy store lock holder did not signal readiness: {status}");
        }
        thread::sleep(Duration::from_millis(20));
    }
}

fn http_command(control_path: &Path, tenant_path: &Path, allow_upgrade: bool) -> Command {
    let control_url = format!("surrealkv://{}", control_path.display());
    let tenant_url = format!("surrealkv://{}", tenant_path.display());
    let mut command = Command::new(HTTP_BINARY);
    command
        .arg("surrealdb-v4-maintenance")
        .env_clear()
        .env("MEMORY_MCP_HTTP_PUBLIC_BASE_URL", "http://localhost")
        .env("MEMORY_MCP_HTTP_AUTH_METHODS", "oidc")
        .env("MEMORY_MCP_HTTP_OIDC_ISSUER", "https://issuer.invalid/")
        .env("MEMORY_MCP_HTTP_OIDC_CLIENT_ID", "test-client")
        .env(
            "MEMORY_MCP_HTTP_SECRET_KEY",
            "0123456789012345678901234567890123456789012345678901234567890123",
        )
        .env("ALLOWED_HOSTS", "localhost")
        .env("ALLOWED_ORIGINS", "http://localhost")
        .env("SURREALDB_CONTROL_URL", control_url)
        .env("SURREALDB_CONTROL_USERNAME", "root")
        .env("SURREALDB_CONTROL_PASSWORD", "root")
        .env("SURREALDB_CONTROL_NAMESPACE", NAMESPACE)
        .env("SURREALDB_CONTROL_DB", DATABASE)
        .env("SURREALDB_TENANT_URL", tenant_url)
        .env("SURREALDB_TENANT_USERNAME", "root")
        .env("SURREALDB_TENANT_PASSWORD", "root")
        .env("SURREALDB_TENANT_NAMESPACE", "tenant")
        .env("SURREALDB_TENANT_DB", DATABASE)
        .env(
            "SURREAL_EXPERIMENTAL_UPGRADE_FROM_3X",
            allow_upgrade.to_string(),
        )
        .env("RUST_LOG", "info");
    command
}

fn interrupt_at_second_migration_checkpoint(mut command: Command) -> String {
    command
        .env("RUST_LOG", "trace")
        .env("MEMORY_LOG_TARGETS", "surrealdb::core::kvs::migration=info");
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start conversion child");
    let stdout = child.stdout.take().expect("captured stdout");
    let stderr = child.stderr.take().expect("captured stderr");
    let stdout_reader = thread::spawn(move || read_pipe(stdout));
    let (sender, receiver) = std::sync::mpsc::channel();
    let stderr_reader = thread::spawn(move || {
        for line in io::BufReader::new(stderr).lines().map_while(Result::ok) {
            let _ = sender.send(line);
        }
    });
    let deadline = Instant::now() + PROCESS_TIMEOUT;
    let mut observed = Vec::new();
    let mut migrations = 0;

    loop {
        match receiver.recv_timeout(Duration::from_millis(100)) {
            Ok(line) => {
                if line.contains("Applying data migration") {
                    migrations += 1;
                    observed.push(line);
                    if migrations == 2 {
                        child
                            .kill()
                            .expect("interrupt at second conversion checkpoint");
                        let status = child.wait().expect("reap interrupted conversion child");
                        let _ = stdout_reader.join().expect("join conversion stdout reader");
                        stderr_reader.join().expect("join conversion stderr reader");
                        assert!(
                            !status.success(),
                            "conversion unexpectedly completed at the interruption checkpoint"
                        );
                        return observed.join("\n");
                    }
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                let status = child.try_wait().expect("check conversion child status");
                let stdout = stdout_reader.join().expect("join conversion stdout reader");
                stderr_reader.join().expect("join conversion stderr reader");
                panic!(
                    "conversion child exited before the second migration checkpoint: {status:?}; lines={observed:?}; stdout={}",
                    String::from_utf8_lossy(&stdout)
                );
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                if let Some(status) = child.try_wait().expect("check conversion child status") {
                    let stdout = stdout_reader.join().expect("join conversion stdout reader");
                    stderr_reader.join().expect("join conversion stderr reader");
                    panic!(
                        "conversion child exited before the second migration checkpoint: {status}; lines={observed:?}; stdout={}",
                        String::from_utf8_lossy(&stdout)
                    );
                }
                if Instant::now() >= deadline {
                    child.kill().expect("kill conversion child after timeout");
                    let status = child.wait().expect("reap timed-out conversion child");
                    let stdout = stdout_reader.join().expect("join conversion stdout reader");
                    stderr_reader.join().expect("join conversion stderr reader");
                    panic!(
                        "conversion missed its second migration checkpoint before {PROCESS_TIMEOUT:?}: {status}; lines={observed:?}; stdout={}",
                        String::from_utf8_lossy(&stdout)
                    );
                }
            }
        }
    }
}

fn run_bounded(mut command: Command, timeout: Duration) -> Output {
    let mut child = command
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start bounded child process");
    let stdout = child.stdout.take().expect("captured stdout");
    let stderr = child.stderr.take().expect("captured stderr");
    let stdout_reader = thread::spawn(move || read_pipe(stdout));
    let stderr_reader = thread::spawn(move || read_pipe(stderr));
    let deadline = Instant::now() + timeout;

    let status = loop {
        match child.try_wait().expect("poll child process") {
            Some(status) => break status,
            None if Instant::now() >= deadline => {
                child.kill().expect("kill timed-out child");
                let status = child.wait().expect("reap timed-out child");
                let stdout = stdout_reader.join().expect("join stdout reader");
                let stderr = stderr_reader.join().expect("join stderr reader");
                panic!(
                    "child exceeded {timeout:?}; status={status}; stdout={} stderr={}",
                    String::from_utf8_lossy(&stdout),
                    String::from_utf8_lossy(&stderr)
                );
            }
            None => thread::sleep(Duration::from_millis(20)),
        }
    };

    Output {
        status,
        stdout: stdout_reader.join().expect("join stdout reader"),
        stderr: stderr_reader.join().expect("join stderr reader"),
    }
}

fn read_pipe(mut pipe: impl io::Read) -> Vec<u8> {
    let mut bytes = Vec::new();
    pipe.read_to_end(&mut bytes).expect("read child output");
    bytes
}

fn assert_success(output: Output, operation: &str) {
    assert!(
        output.status.success(),
        "{operation} failed with {}: stdout={} stderr={}",
        output.status,
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_refusal_is_actionable(engine: LegacyEngine) {
    let directory = TempDir::new().expect("temporary test root");
    let store = directory.path().join("legacy-store");
    seed_fixture(engine, &store);
    let before = hash_tree(&store).expect("hash legacy store before refusal");

    let output = run_bounded(primary_command(&store, false), PROCESS_TIMEOUT);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "legacy v3 store must be refused");
    assert!(
        stderr.contains("backup"),
        "missing backup guidance: {stderr}"
    );
    assert!(
        stderr.contains("SURREAL_EXPERIMENTAL_UPGRADE_FROM_3X") && stderr.contains("true"),
        "missing exact opt-in guidance: {stderr}"
    );

    let after = hash_tree(&store).expect("hash legacy store after refusal");
    assert_eq!(before, after, "refusal changed legacy store file checksums");
}

#[test]
#[ignore = "requires the pinned SurrealDB 3.3.5 fixture runner"]
fn refuses_legacy_rocksdb_without_opt_in_or_file_changes() {
    assert_refusal_is_actionable(LegacyEngine::RocksDb);
}

#[test]
#[ignore = "requires the pinned SurrealDB 3.3.5 fixture runner"]
fn refuses_legacy_surrealkv_without_opt_in_or_file_changes() {
    assert_refusal_is_actionable(LegacyEngine::SurrealKv);
}

#[tokio::test]
#[ignore = "requires the pinned SurrealDB 3.3.5 fixture runner"]
async fn interrupted_primary_rocksdb_conversion_resumes_and_restarts() {
    assert_interrupted_primary_conversion(LegacyEngine::RocksDb).await;
}

#[tokio::test]
#[ignore = "requires the pinned SurrealDB 3.3.5 fixture runner"]
async fn interrupted_primary_surrealkv_conversion_resumes_and_restarts() {
    assert_interrupted_primary_conversion(LegacyEngine::SurrealKv).await;
}

async fn assert_interrupted_primary_conversion(engine: LegacyEngine) {
    let directory = TempDir::new().expect("temporary test root");
    let store = directory.path().join("legacy-store");
    let backup = directory.path().join("cold-backup");
    seed_fixture(engine, &store);
    copy_tree(&store, &backup).expect("make cold backup copy");

    let checkpoint = interrupt_at_second_migration_checkpoint(primary_command(&store, true));
    assert!(checkpoint.contains("Applying data migration"));
    let resumed = run_bounded(primary_command(&store, true), PROCESS_TIMEOUT);
    assert_success(resumed, "resume interrupted primary conversion");
    let restart = run_bounded(primary_command(&store, false), PROCESS_TIMEOUT);
    assert_success(restart, "restart primary after resumed conversion");
    assert_primary_record(&store, false).await;
    verify_fixture(engine, &backup);
}

#[tokio::test]
#[ignore = "requires the pinned SurrealDB 3.3.5 fixture runner"]
async fn interrupted_http_rocksdb_conversion_resumes_and_restarts() {
    assert_interrupted_http_conversion(LegacyEngine::RocksDb).await;
}

#[tokio::test]
#[ignore = "requires the pinned SurrealDB 3.3.5 fixture runner"]
async fn interrupted_http_surrealkv_conversion_resumes_and_restarts() {
    assert_interrupted_http_conversion(LegacyEngine::SurrealKv).await;
}

async fn assert_interrupted_http_conversion(engine: LegacyEngine) {
    let directory = TempDir::new().expect("temporary test root");
    let control = directory.path().join("legacy-control");
    let tenant = directory.path().join("legacy-tenant");
    let control_backup = directory.path().join("control-backup");
    let tenant_backup = directory.path().join("tenant-backup");
    seed_fixture(engine, &control);
    seed_fixture(engine, &tenant);
    copy_tree(&control, &control_backup).expect("back up control store");
    copy_tree(&tenant, &tenant_backup).expect("back up tenant store");

    let checkpoint =
        interrupt_at_second_migration_checkpoint(http_command(&control, &tenant, true));
    assert!(checkpoint.contains("Applying data migration"));
    let resumed = run_bounded(http_command(&control, &tenant, true), PROCESS_TIMEOUT);
    assert_success(resumed, "resume interrupted HTTP conversion");
    let restart = run_bounded(http_command(&control, &tenant, false), PROCESS_TIMEOUT);
    assert_success(restart, "restart HTTP maintenance after conversion");
    assert_primary_record(&control, false).await;
    assert_primary_record(&tenant, false).await;
    verify_fixture(engine, &control_backup);
    verify_fixture(engine, &tenant_backup);
}

#[tokio::test]
#[ignore = "requires the pinned SurrealDB 3.3.5 fixture runner"]
async fn converts_legacy_rocksdb_and_restores_the_cold_copy_with_v3_sdk() {
    assert_converts_and_restores(LegacyEngine::RocksDb).await;
}

#[tokio::test]
#[ignore = "requires the pinned SurrealDB 3.3.5 fixture runner"]
async fn converts_legacy_surrealkv_and_restores_the_cold_copy_with_v3_sdk() {
    assert_converts_and_restores(LegacyEngine::SurrealKv).await;
}

async fn assert_converts_and_restores(engine: LegacyEngine) {
    let directory = TempDir::new().expect("temporary test root");
    let store = directory.path().join("legacy-store");
    let backup = directory.path().join("cold-backup");
    seed_fixture(engine, &store);
    copy_tree(&store, &backup).expect("make cold backup copy");

    let converted = run_bounded(primary_command(&store, true), PROCESS_TIMEOUT);
    assert_success(converted, "opted-in primary conversion");

    let restart = run_bounded(primary_command(&store, false), PROCESS_TIMEOUT);
    assert_success(restart, "primary restart without conversion opt-in");
    assert_primary_record(&store, false).await;
    verify_fixture(engine, &backup);
}

#[tokio::test]
#[ignore = "requires the pinned SurrealDB 3.3.5 fixture runner"]
async fn http_registry_and_tenant_alias_convert_legacy_rocksdb_once() {
    assert_http_shared_store_conversion(LegacyEngine::RocksDb).await;
}

#[tokio::test]
#[ignore = "requires the pinned SurrealDB 3.3.5 fixture runner"]
async fn http_registry_and_tenant_alias_convert_legacy_surrealkv_once() {
    assert_http_shared_store_conversion(LegacyEngine::SurrealKv).await;
}

async fn assert_http_shared_store_conversion(engine: LegacyEngine) {
    let directory = TempDir::new().expect("temporary test root");
    let store = directory.path().join("shared-control-and-tenant");
    let backup = directory.path().join("cold-backup");
    seed_fixture(engine, &store);
    copy_tree(&store, &backup).expect("make cold backup copy");

    let converted = run_bounded(http_command(&store, &store, true), PROCESS_TIMEOUT);
    assert_success(converted, "HTTP conversion with shared-engine alias");
    let restart = run_bounded(http_command(&store, &store, false), PROCESS_TIMEOUT);
    assert_success(restart, "HTTP restart without conversion opt-in");
    assert_primary_record(&store, false).await;
    verify_fixture(engine, &backup);
}

#[tokio::test]
#[ignore = "requires the pinned SurrealDB 3.3.5 fixture runner"]
async fn partial_http_conversion_reports_resume_or_coordinated_restore() {
    let directory = TempDir::new().expect("temporary test root");
    let control = directory.path().join("legacy-control");
    let tenant = directory.path().join("legacy-tenant");
    let ready_file = directory.path().join("tenant-locked");
    seed_fixture(LegacyEngine::RocksDb, &control);
    seed_fixture(LegacyEngine::SurrealKv, &tenant);
    let holder = hold_legacy_store(LegacyEngine::SurrealKv, &tenant, &ready_file);

    let output = run_bounded(http_command(&control, &tenant, true), PROCESS_TIMEOUT);
    drop(holder);
    let transcript = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "locked tenant store must stop HTTP maintenance"
    );
    assert_primary_record(&control, false).await;
    assert!(
        transcript.contains("partial conversion possible"),
        "missing partial-conversion warning: {transcript}"
    );
    assert!(
        transcript.contains("SURREAL_EXPERIMENTAL_UPGRADE_FROM_3X=true"),
        "missing conversion-resume instruction: {transcript}"
    );
    assert!(
        transcript.contains("restore") && transcript.contains("backup"),
        "missing coordinated restore instruction: {transcript}"
    );
}

#[test]
#[ignore = "requires the pinned SurrealDB 3.3.5 fixture runner"]
fn http_registry_refusal_is_actionable_and_preserves_shared_store() {
    let directory = TempDir::new().expect("temporary test root");
    let store = directory.path().join("legacy-control-and-tenant");
    seed_fixture(LegacyEngine::RocksDb, &store);
    let before = hash_tree(&store).expect("hash legacy shared store before refusal");

    let output = run_bounded(http_command(&store, &store, false), PROCESS_TIMEOUT);
    let transcript = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        !output.status.success(),
        "HTTP startup must refuse the legacy store"
    );
    assert!(
        transcript.contains("backup"),
        "missing backup guidance: {transcript}"
    );
    assert!(
        transcript.contains("SURREAL_EXPERIMENTAL_UPGRADE_FROM_3X") && transcript.contains("true"),
        "missing exact opt-in guidance: {transcript}"
    );

    let after = hash_tree(&store).expect("hash legacy shared store after refusal");
    assert_eq!(
        before, after,
        "HTTP refusal changed legacy store file checksums"
    );
}
