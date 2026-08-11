use std::sync::Arc;
use std::time::Instant;

use criterion::{Criterion, criterion_group, criterion_main};
use memory_mcp::service::capabilities::extract::ExtractCapability;

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
                    let _ = ExtractCapability::extract(
                        &service.build_context(),
                        &episode_id,
                        None,
                        None,
                    )
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
                                let _ = ExtractCapability::extract(
                                    &svc.build_context(),
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

criterion_group!(
    benches,
    bench_contention_single_client,
    bench_contention_multi_client
);
criterion_main!(benches);
