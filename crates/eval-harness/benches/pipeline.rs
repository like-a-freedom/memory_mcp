use criterion::{Criterion, criterion_group, criterion_main};
use memory_mcp::service::capabilities::assemble_context::AssembleContextCapability;
use memory_mcp::service::capabilities::extract::ExtractCapability;
use memory_mcp::service::capabilities::ingest::IngestCapability;

fn bench_ingest(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("ingest_single_episode", |b| {
        b.iter(|| {
            rt.block_on(async {
                let service = eval_harness::test_support::make_service().await;
                IngestCapability::ingest(
                    &service.build_context(),
                    memory_mcp::models::IngestRequest {
                        source_type: "bench".into(),
                        source_id: "bench-001".into(),
                        content: "Benchmark test content for ingestion timing.".into(),
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
            });
        });
    });
}

fn bench_extract(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("extract_single_episode", |b| {
        b.iter_batched(
            || {
                rt.block_on(async {
                    let service = eval_harness::test_support::make_service().await;
                    let episode_id = IngestCapability::ingest(
                        &service.build_context(),

                            memory_mcp::models::IngestRequest {
                                source_type: "bench".into(),
                                source_id: "bench-ext-001".into(),
                                content: "The quarterly revenue report shows significant growth across all regions.".into(),
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
                    ExtractCapability::extract(&service.build_context(), &episode_id, None, None).await.unwrap();
                });
            },
            criterion::BatchSize::SmallInput,
        );
    });
}

fn bench_retrieval(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();

    c.bench_function("assemble_context_single_query", |b| {
        b.iter(|| {
            rt.block_on(async {
                let service = eval_harness::test_support::make_service().await;
                for i in 0..10 {
                    IngestCapability::ingest(
                        &service.build_context(),
                        memory_mcp::models::IngestRequest {
                            source_type: "bench".into(),
                            source_id: format!("bench-ret-{i}"),
                            content: format!(
                                "Fact {i}: The project status update shows milestone {i} completed."
                            ),
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
                }
                AssembleContextCapability::assemble_context(
                    &service.build_context(),
                    memory_mcp::models::AssembleContextRequest {
                        query: "project status".into(),
                        scope: "org".into(),
                        as_of: Some(chrono::Utc::now()),
                        budget: 5,
                        project: None,
                        fact_types: vec![],
                        view_mode: None,
                        window_start: None,
                        window_end: None,
                        access: None,
                        compact: false,
                    },
                )
                .await
                .unwrap();
            });
        });
    });
}

fn bench_metrics(c: &mut Criterion) {
    use std::num::NonZeroUsize;

    let observations: Vec<eval_harness::metrics::RetrievalObservation> = (0..100)
        .map(|i| eval_harness::metrics::RetrievalObservation {
            relevant_ids: [format!("relevant-{i}")].into_iter().collect(),
            ranked_ids: (0..20)
                .map(|j| {
                    if j == 0 {
                        format!("relevant-{i}")
                    } else {
                        format!("noise-{j}")
                    }
                })
                .collect(),
        })
        .collect();

    c.bench_function("retrieval_metrics_100_cases", |b| {
        b.iter(|| {
            eval_harness::metrics::retrieval_metrics(&observations, NonZeroUsize::new(5).unwrap())
                .unwrap();
        });
    });
}

criterion_group!(
    benches,
    bench_ingest,
    bench_extract,
    bench_retrieval,
    bench_metrics
);
criterion_main!(benches);
