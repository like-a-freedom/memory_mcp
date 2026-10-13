use std::alloc::{GlobalAlloc, Layout, System};
use std::error::Error;
use std::io;
use std::process::{Child, Command, Output, Stdio};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde::{Deserialize, Serialize};
use surrealdb::Surreal;
use surrealdb::engine::remote::ws::{Client, Ws};
use surrealdb::opt::auth::Root;

const SURREALDB_IMAGE: &str =
    "surrealdb/surrealdb@sha256:ced63befc2ba46d97b922fa388aafcd6c81778760df2ba4aaae72bc00266d9ba";
const SURREALDB_CONTAINER_PORT: &str = "8000/tcp";
const CHILD_TIMEOUT: Duration = Duration::from_secs(120);
const REPORT_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(50);

pub const CHILD_MODE_ENV: &str = "MEMORY_MCP_STORAGE_ALLOCATION_CHILD";
pub const REPORT_PREFIX: &str = "MEMORY_MCP_STORAGE_ALLOCATION_REPORT=";

static LIVE_ALLOCATED_BYTES: AtomicUsize = AtomicUsize::new(0);
static WINDOW_BASE_BYTES: AtomicUsize = AtomicUsize::new(0);
static WINDOW_PEAK_BYTES: AtomicUsize = AtomicUsize::new(0);

struct CountingSystemAllocator;

#[global_allocator]
static GLOBAL_ALLOCATOR: CountingSystemAllocator = CountingSystemAllocator;

unsafe impl GlobalAlloc for CountingSystemAllocator {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        // SAFETY: this delegates the exact layout to the system allocator.
        let ptr = unsafe { System.alloc(layout) };
        if !ptr.is_null() {
            record_growth(layout.size());
        }
        ptr
    }

    unsafe fn alloc_zeroed(&self, layout: Layout) -> *mut u8 {
        // SAFETY: this delegates the exact layout to the system allocator.
        let ptr = unsafe { System.alloc_zeroed(layout) };
        if !ptr.is_null() {
            record_growth(layout.size());
        }
        ptr
    }

    unsafe fn dealloc(&self, ptr: *mut u8, layout: Layout) {
        // SAFETY: the caller of GlobalAlloc must pass the pointer and layout
        // returned by this allocator, and System owns the corresponding block.
        unsafe { System.dealloc(ptr, layout) };
        record_shrink(layout.size());
    }

    unsafe fn realloc(&self, ptr: *mut u8, layout: Layout, new_size: usize) -> *mut u8 {
        // SAFETY: the caller provides a live allocation and its original layout;
        // System performs the requested reallocation.
        let replacement = unsafe { System.realloc(ptr, layout, new_size) };
        if !replacement.is_null() {
            if new_size >= layout.size() {
                record_growth(new_size - layout.size());
            } else {
                record_shrink(layout.size() - new_size);
            }
        }
        replacement
    }
}

fn record_growth(bytes: usize) {
    let previous = LIVE_ALLOCATED_BYTES.fetch_add(bytes, Ordering::Relaxed);
    WINDOW_PEAK_BYTES.fetch_max(previous.saturating_add(bytes), Ordering::Relaxed);
}

fn record_shrink(bytes: usize) {
    let _ = LIVE_ALLOCATED_BYTES.try_update(Ordering::Relaxed, Ordering::Relaxed, |live| {
        Some(live.saturating_sub(bytes))
    });
}

fn begin_allocation_window() -> usize {
    let baseline = LIVE_ALLOCATED_BYTES.load(Ordering::Acquire);
    WINDOW_BASE_BYTES.store(baseline, Ordering::Release);
    WINDOW_PEAK_BYTES.store(baseline, Ordering::Release);
    baseline
}

fn allocation_window_snapshot(baseline: usize) -> (usize, usize) {
    (
        WINDOW_PEAK_BYTES.load(Ordering::Acquire),
        LIVE_ALLOCATED_BYTES
            .load(Ordering::Acquire)
            .saturating_sub(baseline),
    )
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct AllocationReport {
    pub rows: usize,
    pub dimension: usize,
    pub baseline_live_bytes: usize,
    pub peak_delta_bytes: usize,
    pub after_drop_delta_bytes: usize,
    pub query_duration_ms: u64,
}

pub struct StorageMemoryFixture {
    _container: DockerContainer,
    pub url: String,
    pub namespace: String,
    pub database: String,
    dimension: usize,
}

impl StorageMemoryFixture {
    pub async fn start(rows: usize, dimension: usize) -> Result<Self, Box<dyn Error>> {
        if rows == 0 || dimension == 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "rows and dimension must be positive",
            )
            .into());
        }

        let token = SystemTime::now().duration_since(UNIX_EPOCH)?.as_nanos();
        let namespace = format!("allocation_probe_{}", std::process::id());
        let database = format!("allocation_probe_{token}");
        let container = DockerContainer::start()?;
        let port = container.published_port()?;
        let fixture = Self {
            _container: container,
            url: format!("127.0.0.1:{port}"),
            namespace,
            database,
            dimension,
        };

        let client = connect_adapter_retry(
            &fixture.url,
            &fixture.namespace,
            &fixture.database,
            "root",
            "root",
            dimension,
        )
        .await?;
        client.apply_migrations(&fixture.namespace).await?;

        let mut sql = String::new();
        for index in 0..rows {
            sql.push_str(&format!(
                "CREATE fact:allocation_probe_{index:04} CONTENT {{ \
                 fact_id: 'fact:allocation_probe_{index:04}', \
                 fact_type: 'note', \
                 content: 'synthetic allocation probe {index}', \
                 quote: 'synthetic allocation probe {index}', \
                 source_episode: 'episode:seed', \
                 t_valid: time::now(), \
                 t_ingested: time::now(), \
                 confidence: 0.9, \
                 index_keys: [], \
                 access_count: 0, \
                 entity_links: [], \
                 scope: 'org', \
                 policy_tags: [], \
                 provenance: {{ source_episode: 'episode:seed' }}, \
                 embedding: $embedding, \
                 embedding_provider: 'allocation-probe', \
                 embedding_model: 'allocation-probe-v1', \
                 embedding_dimension: {dimension}, \
                 embedding_signature: 'allocation-probe-v1', \
                 embedding_updated_at: time::now(), \
                 t_invalid: NONE \
                 }} RETURN NONE;\n"
            ));
        }

        let vars = serde_json::json!({
            "embedding": vec![0.125_f64; dimension],
        });
        client.query(&sql, Some(vars), &fixture.namespace).await?;

        let count = client
            .query(
                "SELECT count() AS row_count FROM fact GROUP ALL;",
                None,
                &fixture.namespace,
            )
            .await?;
        let seeded_rows = count
            .as_array()
            .and_then(|items| items.first())
            .and_then(|row| row.get("row_count"))
            .and_then(serde_json::Value::as_u64)
            .and_then(|value| usize::try_from(value).ok())
            .unwrap_or_default();
        if seeded_rows != rows {
            return Err(io::Error::other(format!(
                "fixture seeded {seeded_rows} rows, expected {rows}"
            ))
            .into());
        }

        Ok(fixture)
    }

    pub async fn query_json(&self, sql: &str) -> Result<serde_json::Value, Box<dyn Error>> {
        let client = connect_adapter_retry(
            &self.url,
            &self.namespace,
            &self.database,
            "root",
            "root",
            self.dimension,
        )
        .await?;
        Ok(client.query(sql, None, &self.namespace).await?)
    }
}

pub fn run_remote_allocation_case(
    fixture: &StorageMemoryFixture,
    expected_rows: usize,
    dimension: usize,
) -> Result<AllocationReport, Box<dyn Error>> {
    let executable = std::env::current_exe()?;
    let child = Command::new(executable)
        .args([
            "--exact",
            "allocation_child_worker",
            "--ignored",
            "--nocapture",
            "--test-threads=1",
        ])
        .env(CHILD_MODE_ENV, "1")
        .env("MEMORY_MCP_STORAGE_ALLOCATION_URL", &fixture.url)
        .env(
            "MEMORY_MCP_STORAGE_ALLOCATION_NAMESPACE",
            &fixture.namespace,
        )
        .env("MEMORY_MCP_STORAGE_ALLOCATION_DATABASE", &fixture.database)
        .env("MEMORY_MCP_STORAGE_ALLOCATION_USERNAME", "root")
        .env("MEMORY_MCP_STORAGE_ALLOCATION_PASSWORD", "root")
        .env(
            "MEMORY_MCP_STORAGE_ALLOCATION_EXPECTED_ROWS",
            expected_rows.to_string(),
        )
        .env(
            "MEMORY_MCP_STORAGE_ALLOCATION_DIMENSION",
            dimension.to_string(),
        )
        .env("MALLOC_ARENA_MAX", "2")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()?;

    let output = wait_for_child(child, CHILD_TIMEOUT)?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "allocation child exited with {}:\nstdout:\n{}\nstderr:\n{}",
            output.status,
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        ))
        .into());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let Some(report_json) = stdout.lines().find_map(|line| {
        let marker = line.find(REPORT_PREFIX)?;
        Some(&line[marker + REPORT_PREFIX.len()..])
    }) else {
        return Err(io::Error::other(format!(
            "allocation child produced no report:\nstdout:\n{stdout}\nstderr:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ))
        .into());
    };
    Ok(serde_json::from_str(report_json)?)
}

pub async fn measure_remote_query_from_environment() -> Result<AllocationReport, Box<dyn Error>> {
    let url = required_env("MEMORY_MCP_STORAGE_ALLOCATION_URL")?;
    let namespace = required_env("MEMORY_MCP_STORAGE_ALLOCATION_NAMESPACE")?;
    let database = required_env("MEMORY_MCP_STORAGE_ALLOCATION_DATABASE")?;
    let username = required_env("MEMORY_MCP_STORAGE_ALLOCATION_USERNAME")?;
    let password = required_env("MEMORY_MCP_STORAGE_ALLOCATION_PASSWORD")?;
    let expected_rows = parse_env::<usize>("MEMORY_MCP_STORAGE_ALLOCATION_EXPECTED_ROWS")?;
    let expected_dimension = parse_env::<usize>("MEMORY_MCP_STORAGE_ALLOCATION_DIMENSION")?;

    let client = connect_adapter(
        &url,
        &namespace,
        &database,
        &username,
        &password,
        expected_dimension,
    )
    .await?;
    let baseline_live_bytes = begin_allocation_window();
    let started_at = Instant::now();
    let result = client
        .query("SELECT * FROM fact;", None, &namespace)
        .await?;
    let query_duration_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);

    let rows = result.as_array().map_or(0, Vec::len);
    let dimension = result
        .as_array()
        .and_then(|items| items.first())
        .and_then(|row| row.get("embedding"))
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    let all_dimensions_match = result.as_array().is_some_and(|items| {
        items.iter().all(|row| {
            row.get("embedding")
                .and_then(serde_json::Value::as_array)
                .is_some_and(|vector| vector.len() == expected_dimension)
        })
    });
    if rows != expected_rows || dimension != expected_dimension || !all_dimensions_match {
        return Err(io::Error::other(format!(
            "queried payload shape was rows={rows}, dimension={dimension}, all_dimensions_match={all_dimensions_match}; expected rows={expected_rows}, dimension={expected_dimension}"
        ))
        .into());
    }

    drop(result);
    let (peak_live_bytes, after_drop_delta_bytes) = allocation_window_snapshot(baseline_live_bytes);

    Ok(AllocationReport {
        rows,
        dimension,
        baseline_live_bytes,
        peak_delta_bytes: peak_live_bytes.saturating_sub(baseline_live_bytes),
        after_drop_delta_bytes,
        query_duration_ms,
    })
}

async fn connect_adapter(
    url: &str,
    namespace: &str,
    database: &str,
    username: &str,
    password: &str,
    dimension: usize,
) -> Result<SurrealDbClient, Box<dyn Error>> {
    let db: Surreal<Client> = Surreal::new::<Ws>(url).await?;
    db.signin(Root {
        username: username.to_string(),
        password: password.to_string(),
    })
    .await?;
    db.use_ns(namespace).use_db(database).await?;

    Ok(SurrealDbClient::from_prebound_remote_with_dimension(
        db, namespace, "error", dimension,
    ))
}

async fn connect_adapter_retry(
    url: &str,
    namespace: &str,
    database: &str,
    username: &str,
    password: &str,
    dimension: usize,
) -> Result<SurrealDbClient, Box<dyn Error>> {
    let deadline = Instant::now() + REPORT_TIMEOUT;

    loop {
        let error =
            match connect_adapter(url, namespace, database, username, password, dimension).await {
                Ok(client) => return Ok(client),
                Err(error) => error.to_string(),
            };

        if Instant::now() >= deadline {
            return Err(io::Error::new(
                io::ErrorKind::TimedOut,
                format!(
                    "SurrealDB did not accept an authenticated connection within {REPORT_TIMEOUT:?}: {error}"
                ),
            )
            .into());
        }

        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

fn required_env(name: &str) -> Result<String, io::Error> {
    std::env::var(name).map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("required environment variable {name} is unavailable: {error}"),
        )
    })
}

fn parse_env<T>(name: &str) -> Result<T, io::Error>
where
    T: std::str::FromStr,
    T::Err: Error + Send + Sync + 'static,
{
    required_env(name)?.parse().map_err(|error| {
        io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("environment variable {name} is invalid: {error}"),
        )
    })
}

fn wait_for_child(mut child: Child, timeout: Duration) -> io::Result<Output> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait()? {
            Some(_) => return child.wait_with_output(),
            None if Instant::now() < deadline => std::thread::sleep(POLL_INTERVAL),
            None => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    format!("allocation child exceeded {timeout:?}"),
                ));
            }
        }
    }
}

struct DockerContainer {
    name: String,
}

impl DockerContainer {
    fn start() -> io::Result<Self> {
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        let name = format!("memory-mcp-storage-{}-{timestamp}", std::process::id());
        let output = Command::new("docker")
            .args([
                "run",
                "--detach",
                "--rm",
                "--name",
                &name,
                "--publish",
                "127.0.0.1::8000",
                SURREALDB_IMAGE,
                "start",
                "--no-banner",
                "--bind",
                "0.0.0.0:8000",
                "--username",
                "root",
                "--password",
                "root",
                "--allow-experimental",
                "files",
                "--log",
                "error",
                "memory",
            ])
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "could not start local SurrealDB test container: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        Ok(Self { name })
    }

    fn published_port(&self) -> io::Result<u16> {
        let output = Command::new("docker")
            .args(["port", &self.name, SURREALDB_CONTAINER_PORT])
            .output()?;
        if !output.status.success() {
            return Err(io::Error::other(format!(
                "could not read SurrealDB test container port: {}",
                String::from_utf8_lossy(&output.stderr)
            )));
        }

        let mapping = String::from_utf8_lossy(&output.stdout);
        let port_text = mapping
            .lines()
            .find_map(|line| line.trim().rsplit_once(':').map(|(_, port)| port))
            .ok_or_else(|| io::Error::other("Docker published no TCP host port"))?;
        port_text
            .parse()
            .map_err(|error| io::Error::other(format!("invalid Docker host port: {error}")))
    }
}

impl Drop for DockerContainer {
    fn drop(&mut self) {
        let _ = Command::new("docker")
            .args(["rm", "--force", &self.name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}
