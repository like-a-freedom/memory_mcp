use criterion::{Criterion, black_box, criterion_group, criterion_main};
use memory_mcp::service::capabilities::extract::ExtractCapability;
use memory_mcp::service::capabilities::ingest::IngestCapability;

fn bench_ner_cpu_single_window(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("ner_cpu_single_window", |b| {
        b.iter(|| {
            rt.block_on(async {
                let service = eval_harness::test_support::make_service().await;
                let episode_id = IngestCapability::ingest(
                    &service.build_context(),

                        memory_mcp::models::IngestRequest {
                            source_type: "bench".into(),
                            source_id: "ner-bench-001".into(),
                            content: "Alice Smith from Acme Corp presented the quarterly revenue report showing $5.2M in ARR.".into(),
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
                black_box(ExtractCapability::extract(&service.build_context(), &episode_id, None, None).await.unwrap());
            });
        });
    });
}

fn bench_ner_cpu_multi_window(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("ner_cpu_multi_window", |b| {
        b.iter_batched(
            || {
                rt.block_on(async {
                    let service = eval_harness::test_support::make_service().await;
                    let content: String = (0..10)
                        .map(|i| format!("Window {i}: Alice Smith from Acme Corp reported revenue milestone {i}."))
                        .collect::<Vec<_>>()
                        .join(". ");
                    let episode_id = IngestCapability::ingest(
                        &service.build_context(),

                            memory_mcp::models::IngestRequest {
                                source_type: "bench".into(),
                                source_id: "ner-bench-multi".into(),
                                content,
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
                    (service, episode_id)
                })
            },
            |(service, episode_id)| {
                rt.block_on(async {
                    black_box(ExtractCapability::extract(&service.build_context(), &episode_id, None, None).await.unwrap());
                });
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

criterion_group!(
    benches,
    bench_ner_cpu_single_window,
    bench_ner_cpu_multi_window
);
criterion_main!(benches);
