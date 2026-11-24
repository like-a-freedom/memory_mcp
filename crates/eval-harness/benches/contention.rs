use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use criterion::{Criterion, criterion_group, criterion_main};
use memory_mcp::config::SurrealConfigBuilder;
use memory_mcp::eval_support::{QueryObserver, SqlObservation};
use memory_mcp::models::Provenance;
use memory_mcp::service::MemoryService;
use memory_mcp::service::memory_container_shims::memory_capabilities_extract::ExtractCapability;
use memory_mcp::storage::{DbClient, SurrealDbClient};

#[derive(Default)]
struct RevisionTimingSamples(Mutex<Vec<(u64, u64)>>);

impl QueryObserver for RevisionTimingSamples {
    fn observe(&self, _observation: &SqlObservation) {}

    fn observe_revision_transaction(&self, lock_wait_micros: u64, transaction_micros: u64) {
        self.0
            .lock()
            .expect("revision timing sample lock")
            .push((lock_wait_micros, transaction_micros));
    }
}

impl RevisionTimingSamples {
    fn clear(&self) {
        self.0.lock().expect("revision timing sample lock").clear();
    }
}

async fn write_durable_revision_batch(services: &[Arc<MemoryService>], next_fact: &AtomicUsize) {
    let mut tasks = Vec::with_capacity(services.len());
    for service in services {
        let service = Arc::clone(service);
        let index = next_fact.fetch_add(1, Ordering::Relaxed);
        tasks.push(tokio::spawn(async move {
            let content = format!("Durable revision benchmark fact {index}.");
            service
                .add_fact(
                    "note",
                    &content,
                    &content,
                    &format!("episode:revision-bench-{index}"),
                    chrono::Utc::now(),
                    0.9,
                    Vec::new(),
                    Vec::new(),
                    Provenance::manual(),
                )
                .await
                .expect("persist durable benchmark fact");
        }));
    }
    for task in tasks {
        task.await.expect("join durable benchmark writer");
    }
}

fn bench_contention_single_client(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("contention_single_client", |b| {
        b.iter(|| {
            rt.block_on(async {
                let service = eval_harness::test_support::make_service().await;
                for i in 0..5 {
                    let episode_id = eval_harness::test_support::ingest_probe(
                        &service,
                        &format!("contention-{i}"),
                        &format!(
                            "Fact {i}: Alice Smith from Acme Corp reported revenue milestone {i}."
                        ),
                    )
                    .await;
                    let _ =
                        ExtractCapability::extract_from_service(&service, &episode_id, None, None)
                            .await
                            .unwrap();
                }
            });
        });
    });
}

fn bench_contention_multi_client(c: &mut Criterion) {
    let mut group = c.benchmark_group("contention");
    for num_clients in [2, 4] {
        group.bench_function(format!("clients_{num_clients}"), |b| {
            let rt = tokio::runtime::Runtime::new().unwrap();
            b.iter(|| {
                rt.block_on(async {
                    let service = Arc::new(eval_harness::test_support::make_service().await);
                    let ops_per_client = 3;
                    let total_ops = num_clients * ops_per_client;
                    let start = Instant::now();

                    let mut handles = Vec::new();
                    for client_idx in 0..num_clients {
                        let svc = Arc::clone(&service);
                        handles.push(tokio::spawn(async move {
                            for i in 0..ops_per_client {
                                let episode_id = eval_harness::test_support::ingest_probe(
                                    &svc,
                                    &format!("contention-c{client_idx}-{i}"),
                                    &format!(
                                        "Client {client_idx} fact {i}: Project status update."
                                    ),
                                )
                                .await;
                                let _ = ExtractCapability::extract_from_service(
                                    &svc,
                                    &episode_id,
                                    None,
                                    None,
                                )
                                .await
                                .unwrap();
                            }
                        }));
                    }
                    for h in handles {
                        h.await.unwrap();
                    }

                    let elapsed = start.elapsed();
                    let observation = eval_harness::benchmark::ContentionObservation::new(
                        num_clients,
                        total_ops,
                        elapsed,
                    );
                    eprintln!(
                        "contention clients={} ops={} elapsed={:?} ops/s={:.1} latency/op={:?}",
                        observation.clients,
                        observation.operations,
                        observation.elapsed,
                        observation.ops_per_second(),
                        observation.latency_per_operation()
                    );
                });
            });
        });
    }
    group.finish();
}

fn bench_durable_revision_clock_contention(c: &mut Criterion) {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    let directory = tempfile::tempdir().unwrap();
    let database = runtime.block_on(async {
        let config = SurrealConfigBuilder::new()
            .db_name("revision_clock_bench")
            .namespace("main")
            .credentials("root", "root")
            .log_level("error")
            .embedded(true)
            .data_dir(directory.path().display().to_string())
            .build()
            .expect("build isolated SurrealKV benchmark config");
        let database = Arc::new(
            SurrealDbClient::connect(&config)
                .await
                .expect("open isolated durable SurrealKV database"),
        );
        database
            .apply_migrations("main")
            .await
            .expect("apply production migrations");
        database
    });
    let timings = Arc::new(RevisionTimingSamples::default());
    database.set_eval_query_observer(timings.clone());
    let services = (0..4)
        .map(|_| {
            Arc::new(
                MemoryService::new(
                    database.clone(),
                    "main".to_string(),
                    "error".to_string(),
                    50,
                    100,
                )
                .expect("create durable benchmark service"),
            )
        })
        .collect::<Vec<_>>();
    let next_fact = AtomicUsize::new(0);

    c.bench_function("revision_clock_durable_surreal_kv_4_writers", |benchmark| {
        benchmark.iter(|| {
            runtime.block_on(write_durable_revision_batch(&services, &next_fact));
        });
    });

    runtime.block_on(async {
        for _ in 0..16 {
            write_durable_revision_batch(&services, &next_fact).await;
        }
        timings.clear();
        for _ in 0..100 {
            write_durable_revision_batch(&services, &next_fact).await;
        }
    });
    let samples = timings
        .0
        .lock()
        .expect("revision timing sample lock")
        .clone();
    if samples.is_empty() {
        eprintln!("revision_clock_durable samples=0");
    } else {
        let lock_wait = samples.iter().map(|sample| sample.0).collect::<Vec<_>>();
        let transaction = samples.iter().map(|sample| sample.1).collect::<Vec<_>>();
        eprintln!(
            "revision_clock_durable warmup_batches=16 measured_batches=100 writers=4 samples={} lock_wait_us_p50={} lock_wait_us_p95={} transaction_us_p50={} transaction_us_p95={}",
            samples.len(),
            percentile(&lock_wait, 50),
            percentile(&lock_wait, 95),
            percentile(&transaction, 50),
            percentile(&transaction, 95),
        );
    }
    runtime
        .block_on(database.shutdown())
        .expect("close benchmark database");
}

fn bench_remote_revision_clock_contention(c: &mut Criterion) {
    let Some(config) = RemoteRevisionBenchConfig::from_env() else {
        eprintln!(
            "revision_clock_remote skipped; set MEMORY_REVISION_REMOTE_BENCH_URL to a dedicated loopback SurrealDB server"
        );
        return;
    };

    let runtime = tokio::runtime::Runtime::new().expect("create remote benchmark runtime");
    let namespace = config.namespace.clone();
    let database_name = config.database_name.clone();
    let (databases, services, timings) = runtime.block_on(async {
        let surreal_config = SurrealConfigBuilder::new()
            .url(config.endpoint.clone())
            .db_name(config.database_name.clone())
            .namespace(config.namespace.clone())
            .credentials(config.username.clone(), config.password.clone())
            .log_level("error")
            .embedded(false)
            .build()
            .expect("build isolated remote SurrealDB config");
        let mut databases = Vec::with_capacity(4);
        for _ in 0..4 {
            databases.push(Arc::new(
                SurrealDbClient::connect(&surreal_config)
                    .await
                    .expect("connect independent remote SurrealDB client"),
            ));
        }
        databases[0]
            .apply_migrations(&namespace)
            .await
            .expect("apply isolated remote benchmark migrations");

        let timings = Arc::new(RevisionTimingSamples::default());
        for database in &databases {
            database.set_eval_query_observer(timings.clone());
        }
        let services = databases
            .iter()
            .map(|database| {
                Arc::new(
                    MemoryService::new(
                        database.clone(),
                        namespace.clone(),
                        "error".to_string(),
                        50,
                        100,
                    )
                    .expect("create remote revision benchmark service"),
                )
            })
            .collect::<Vec<_>>();
        (databases, services, timings)
    });
    let next_fact = AtomicUsize::new(0);

    c.bench_function("revision_clock_remote_ws_4_clients", |benchmark| {
        benchmark.iter(|| {
            runtime.block_on(write_durable_revision_batch(&services, &next_fact));
        });
    });

    runtime.block_on(async {
        for _ in 0..16 {
            write_durable_revision_batch(&services, &next_fact).await;
        }
        timings.clear();
        for _ in 0..100 {
            write_durable_revision_batch(&services, &next_fact).await;
        }
    });
    let samples = timings
        .0
        .lock()
        .expect("remote revision timing sample lock")
        .clone();
    if samples.is_empty() {
        eprintln!(
            "revision_clock_remote database={database_name} endpoint={} samples=0",
            config.endpoint
        );
    } else {
        let lock_wait = samples.iter().map(|sample| sample.0).collect::<Vec<_>>();
        let transaction = samples.iter().map(|sample| sample.1).collect::<Vec<_>>();
        eprintln!(
            "revision_clock_remote database={database_name} endpoint={} warmup_batches=16 measured_batches=100 clients=4 samples={} lock_wait_us_p50={} lock_wait_us_p95={} transaction_us_p50={} transaction_us_p95={}",
            config.endpoint,
            samples.len(),
            percentile(&lock_wait, 50),
            percentile(&lock_wait, 95),
            percentile(&transaction, 50),
            percentile(&transaction, 95),
        );
    }
    runtime.block_on(async {
        for database in &databases {
            database
                .shutdown()
                .await
                .expect("close remote benchmark database connection");
        }
    });
}

struct RemoteRevisionBenchConfig {
    endpoint: String,
    username: String,
    password: String,
    namespace: String,
    database_name: String,
}

impl RemoteRevisionBenchConfig {
    fn from_env() -> Option<Self> {
        let endpoint = std::env::var("MEMORY_REVISION_REMOTE_BENCH_URL").ok()?;
        assert_eq!(
            std::env::var("MEMORY_REVISION_REMOTE_BENCH_ACK").as_deref(),
            Ok("dedicated-loopback-database"),
            "remote revision benchmark writes require MEMORY_REVISION_REMOTE_BENCH_ACK=dedicated-loopback-database"
        );
        let address = endpoint
            .strip_prefix("ws://")
            .and_then(|address| address.parse::<std::net::SocketAddr>().ok())
            .filter(|address| address.ip().is_loopback())
            .expect("remote revision benchmark URL must be a ws:// loopback socket address");
        let username = std::env::var("MEMORY_REVISION_REMOTE_BENCH_USERNAME")
            .expect("set MEMORY_REVISION_REMOTE_BENCH_USERNAME");
        let password = std::env::var("MEMORY_REVISION_REMOTE_BENCH_PASSWORD")
            .expect("set MEMORY_REVISION_REMOTE_BENCH_PASSWORD");
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("system clock is after the Unix epoch")
            .as_secs();
        Some(Self {
            endpoint: format!("ws://{address}"),
            username,
            password,
            namespace: format!("revision_bench_{}_{timestamp}", std::process::id()),
            database_name: "revision_clock".to_string(),
        })
    }
}

fn percentile(samples: &[u64], percentile: usize) -> u64 {
    let mut sorted = samples.to_vec();
    sorted.sort_unstable();
    let index = (sorted.len().saturating_sub(1) * percentile).div_ceil(100);
    sorted[index]
}

criterion_group!(
    benches,
    bench_contention_single_client,
    bench_contention_multi_client,
    bench_durable_revision_clock_contention,
    bench_remote_revision_clock_contention
);
criterion_main!(benches);
