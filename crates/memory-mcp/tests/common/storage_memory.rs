use std::alloc::{GlobalAlloc, Layout, System};
use std::error::Error;
use std::io;
use std::process::{Child, Command, Output, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use async_trait::async_trait;
use memory_mcp::error::MemoryError;
use memory_mcp::knowledge::graph_store::KnowledgeGraphStore;
use memory_mcp::storage::{DbClient, SurrealDbClient};
use serde::{Deserialize, Serialize};
use surrealdb::Surreal;
use surrealdb::engine::remote::ws::{Client, Ws};
use surrealdb::opt::auth::Root;

const SURREALDB_IMAGE: &str =
    "surrealdb/surrealdb@sha256:58944c284296507fd3cb343e7568a08dd38f397f7e763f9f54a71c5562c5839f";
const SURREALDB_CONTAINER_PORT: &str = "8000/tcp";
const CHILD_TIMEOUT: Duration = Duration::from_secs(120);
const REPORT_TIMEOUT: Duration = Duration::from_secs(30);
const POLL_INTERVAL: Duration = Duration::from_millis(50);
const ALLOCATION_QUERY_PAGE_ROWS: usize = 300;
static NEXT_DOCKER_CONTAINER_SEQUENCE: AtomicUsize = AtomicUsize::new(0);

pub const CHILD_MODE_ENV: &str = "MEMORY_MCP_STORAGE_ALLOCATION_CHILD";
pub const REPORT_PREFIX: &str = "MEMORY_MCP_STORAGE_ALLOCATION_REPORT=";
const ALLOCATION_MODE_ENV: &str = "MEMORY_MCP_STORAGE_ALLOCATION_MODE";
const BOUND_SCAN_CONTENT_BYTES_ENV: &str = "MEMORY_MCP_STORAGE_BOUND_SCAN_CONTENT_BYTES";
const NO_WINDOW_BASELINE: usize = usize::MAX;

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
        self.query_json_with_vars(sql, None).await
    }

    pub async fn query_json_with_vars(
        &self,
        sql: &str,
        vars: Option<serde_json::Value>,
    ) -> Result<serde_json::Value, Box<dyn Error>> {
        let client = self.connect_adapter().await?;
        Ok(client.query(sql, vars, &self.namespace).await?)
    }

    pub async fn connect_adapter(&self) -> Result<SurrealDbClient, Box<dyn Error>> {
        connect_adapter_retry(
            &self.url,
            &self.namespace,
            &self.database,
            "root",
            "root",
            self.dimension,
        )
        .await
    }
}

pub fn run_remote_allocation_case(
    fixture: &StorageMemoryFixture,
    expected_rows: usize,
    dimension: usize,
) -> Result<AllocationReport, Box<dyn Error>> {
    run_allocation_child(fixture, expected_rows, dimension, "full_query", None)
}

/// Measures the owning knowledge-store handoff after a real adapter query has
/// produced ready JSON rows. The fixture uses a valid positive schema width,
/// but removes every stored vector before the child process begins.
pub async fn run_remote_bound_scan_case(
    expected_rows: usize,
    content_bytes: usize,
) -> Result<AllocationReport, Box<dyn Error>> {
    let fixture = StorageMemoryFixture::start(expected_rows, 1).await?;
    let content = "x".repeat(content_bytes);
    fixture
        .query_json_with_vars(
            "UPDATE fact SET content = $content, embedding = NONE;",
            Some(serde_json::json!({"content": content})),
        )
        .await?;

    run_allocation_child(
        &fixture,
        expected_rows,
        0,
        "bound_active_fact_query",
        Some(content_bytes),
    )
}

/// Measures a full unprojected fact query where the optional embedding field
/// is absent. The database schema keeps a valid positive width; zero is only
/// used as the report's absent-vector sentinel.
pub async fn run_remote_no_embedding_allocation_case(
    expected_rows: usize,
) -> Result<AllocationReport, Box<dyn Error>> {
    let fixture = StorageMemoryFixture::start(expected_rows, 1).await?;
    fixture
        .query_json("UPDATE fact SET embedding = NONE;")
        .await?;

    run_allocation_child(&fixture, expected_rows, 0, "full_query_no_embedding", None)
}

fn run_allocation_child(
    fixture: &StorageMemoryFixture,
    expected_rows: usize,
    dimension: usize,
    mode: &str,
    content_bytes: Option<usize>,
) -> Result<AllocationReport, Box<dyn Error>> {
    let executable = std::env::current_exe()?;
    let mut command = Command::new(executable);
    command
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
        .env(ALLOCATION_MODE_ENV, mode)
        .env("MALLOC_ARENA_MAX", "2")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    if let Some(content_bytes) = content_bytes {
        command.env(BOUND_SCAN_CONTENT_BYTES_ENV, content_bytes.to_string());
    }
    let child = command.spawn()?;

    let output = wait_for_child(child, CHILD_TIMEOUT)?;
    if !output.status.success() {
        return Err(io::Error::other(format!(
            "allocation child mode={mode}, rows={expected_rows}, report_dimension={dimension} exited with {}:\nstdout:\n{}\nstderr:\n{}",
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
        expected_dimension.max(1),
    )
    .await?;
    let baseline_live_bytes = begin_allocation_window();
    let started_at = Instant::now();
    // Keep the complete converted row set alive for the measurement, but bound
    // each WebSocket result page. A 1000-row D3072 response is larger than the
    // pinned server's local WebSocket response limit and resets the socket.
    let mut all_rows = Vec::with_capacity(expected_rows);
    for offset in (0..expected_rows).step_by(ALLOCATION_QUERY_PAGE_ROWS) {
        let sql = format!(
            "SELECT * FROM fact ORDER BY id LIMIT {ALLOCATION_QUERY_PAGE_ROWS} START {offset};"
        );
        let page = client.query(&sql, None, &namespace).await?;
        let mut page_rows = match page {
            serde_json::Value::Array(rows) => rows,
            _ => {
                return Err(io::Error::other(format!(
                    "paged query at offset {offset} returned a non-array result"
                ))
                .into());
            }
        };
        all_rows.append(&mut page_rows);
    }
    let result = serde_json::Value::Array(all_rows);
    let query_duration_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);

    let rows = result.as_array().map_or(0, Vec::len);
    let dimension = result
        .as_array()
        .and_then(|items| items.first())
        .and_then(|row| row.get("embedding"))
        .and_then(serde_json::Value::as_array)
        .map_or(0, Vec::len);
    let all_embedding_shapes_and_samples_match = result.as_array().is_some_and(|items| {
        items.iter().all(|row| {
            if expected_dimension == 0 {
                row.get("embedding").is_none()
            } else {
                row.get("embedding")
                    .and_then(serde_json::Value::as_array)
                    .is_some_and(|vector| {
                        vector.len() == expected_dimension
                            && [0, expected_dimension / 2, expected_dimension - 1]
                                .into_iter()
                                .all(|index| {
                                    vector.get(index).and_then(serde_json::Value::as_f64)
                                        == Some(0.125)
                                })
                    })
            }
        })
    });
    if rows != expected_rows
        || dimension != expected_dimension
        || !all_embedding_shapes_and_samples_match
    {
        return Err(io::Error::other(format!(
            "queried payload shape was rows={rows}, dimension={dimension}, all_embedding_shapes_and_samples_match={all_embedding_shapes_and_samples_match}; expected rows={expected_rows}, dimension={expected_dimension}"
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

pub async fn measure_remote_bound_scan_from_environment() -> Result<AllocationReport, Box<dyn Error>>
{
    let url = required_env("MEMORY_MCP_STORAGE_ALLOCATION_URL")?;
    let namespace = required_env("MEMORY_MCP_STORAGE_ALLOCATION_NAMESPACE")?;
    let database = required_env("MEMORY_MCP_STORAGE_ALLOCATION_DATABASE")?;
    let username = required_env("MEMORY_MCP_STORAGE_ALLOCATION_USERNAME")?;
    let password = required_env("MEMORY_MCP_STORAGE_ALLOCATION_PASSWORD")?;
    let expected_rows = parse_env::<usize>("MEMORY_MCP_STORAGE_ALLOCATION_EXPECTED_ROWS")?;
    let expected_content_bytes = parse_env::<usize>(BOUND_SCAN_CONTENT_BYTES_ENV)?;

    // The server schema has a real positive width, but all measured rows omit
    // their vectors. Zero is only the report sentinel for that absent-field
    // fixture state; no schema or provider is configured with dimension zero.
    let adapter = connect_adapter(&url, &namespace, &database, &username, &password, 1).await?;
    let window_baseline = Arc::new(AtomicUsize::new(NO_WINDOW_BASELINE));
    let measured_db: Arc<dyn DbClient> = Arc::new(QueryRowsWindowClient {
        inner: Arc::new(adapter),
        window_baseline: Arc::clone(&window_baseline),
    });
    let store = KnowledgeGraphStore::new(measured_db, namespace.clone());

    let started_at = Instant::now();
    let rows = store
        .select_active_facts(i32::try_from(expected_rows)?)
        .await?;
    let query_duration_ms = u64::try_from(started_at.elapsed().as_millis()).unwrap_or(u64::MAX);
    let baseline_live_bytes = window_baseline.load(Ordering::Acquire);
    if baseline_live_bytes == NO_WINDOW_BASELINE {
        return Err(io::Error::other(
            "the bound store did not pass a query result through DbClient::query",
        )
        .into());
    }

    let row_count = rows.len();
    let all_rows_match = rows.iter().all(|row| {
        row.get("content")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|content| content.len() == expected_content_bytes)
            && row.get("embedding").is_none()
    });
    drop(rows);

    let (peak_live_bytes, after_drop_delta_bytes) = allocation_window_snapshot(baseline_live_bytes);
    if row_count != expected_rows || !all_rows_match {
        return Err(io::Error::other(format!(
            "bound active fact result shape was rows={row_count}, all_content_lengths_and_vectors_match={all_rows_match}; expected rows={expected_rows}, content_bytes={expected_content_bytes}, vectors absent"
        ))
        .into());
    }

    Ok(AllocationReport {
        rows: row_count,
        dimension: 0,
        baseline_live_bytes,
        peak_delta_bytes: peak_live_bytes.saturating_sub(baseline_live_bytes),
        after_drop_delta_bytes,
        query_duration_ms,
    })
}

struct QueryRowsWindowClient {
    inner: Arc<dyn DbClient>,
    window_baseline: Arc<AtomicUsize>,
}

#[async_trait]
impl DbClient for QueryRowsWindowClient {
    async fn select_one(
        &self,
        record_id: &str,
        namespace: &str,
    ) -> Result<Option<serde_json::Value>, MemoryError> {
        self.inner.select_one(record_id, namespace).await
    }

    async fn select_table(
        &self,
        table: memory_mcp::storage::table_scope::OwnedTable,
        namespace: &str,
    ) -> Result<Vec<serde_json::Value>, MemoryError> {
        self.inner.select_table(table, namespace).await
    }

    async fn create(
        &self,
        record_id: &str,
        content: serde_json::Value,
        namespace: &str,
        temporal_fields: &[&str],
    ) -> Result<serde_json::Value, MemoryError> {
        self.inner
            .create(record_id, content, namespace, temporal_fields)
            .await
    }

    async fn update(
        &self,
        record_id: &str,
        content: serde_json::Value,
        namespace: &str,
        temporal_fields: &[&str],
    ) -> Result<serde_json::Value, MemoryError> {
        self.inner
            .update(record_id, content, namespace, temporal_fields)
            .await
    }

    async fn query(
        &self,
        sql: &str,
        vars: Option<serde_json::Value>,
        namespace: &str,
    ) -> Result<serde_json::Value, MemoryError> {
        let value = self.inner.query(sql, vars, namespace).await?;
        let baseline = begin_allocation_window();
        self.window_baseline.store(baseline, Ordering::Release);
        Ok(value)
    }

    async fn apply_migrations(&self, namespace: &str) -> Result<(), MemoryError> {
        self.inner.apply_migrations(namespace).await
    }
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
        let sequence = NEXT_DOCKER_CONTAINER_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let name = format!("memory-mcp-storage-{}-{sequence}", std::process::id());
        let output = Command::new("docker")
            .args([
                "run",
                "--platform",
                "linux/amd64",
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
