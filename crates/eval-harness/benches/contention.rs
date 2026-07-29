use criterion::{Criterion, black_box, criterion_group, criterion_main};
use std::sync::Arc;

fn bench_contention_single_client(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("contention_single_client", |b| {
        b.iter(|| {
            rt.block_on(async {
                let service = eval_harness::test_support::make_service().await;
                for i in 0..5 {
                    let episode_id = service
                        .ingest(
                            memory_mcp::models::IngestRequest {
                                source_type: "bench".into(),
                                source_id: format!("contention-{i}"),
                                content: format!("Fact {i}: Alice Smith from Acme Corp reported revenue milestone {i}."),
                                t_ref: chrono::Utc::now(),
                                scope: "org".into(),
                                project: None,
                                t_ingested: None,
                                visibility_scope: None,
                                policy_tags: vec![],
                            },
                            None,
                        )
                        .await
                        .unwrap();
                    black_box(service.extract(&episode_id, None, None).await.unwrap());
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
                    let mut handles = Vec::new();
                    for client_idx in 0..num_clients {
                        let svc = Arc::clone(&service);
                        handles.push(tokio::spawn(async move {
                            for i in 0..3 {
                                let episode_id = svc
                                    .ingest(
                                        memory_mcp::models::IngestRequest {
                                            source_type: "bench".into(),
                                            source_id: format!("contention-c{client_idx}-{i}"),
                                            content: format!("Client {client_idx} fact {i}: Project status update."),
                                            t_ref: chrono::Utc::now(),
                                            scope: "org".into(),
                                            project: None,
                                            t_ingested: None,
                                            visibility_scope: None,
                                            policy_tags: vec![],
                                        },
                                        None,
                                    )
                                    .await
                                    .unwrap();
                                black_box(svc.extract(&episode_id, None, None).await.unwrap());
                            }
                        }));
                    }
                    for h in handles {
                        h.await.unwrap();
                    }
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
