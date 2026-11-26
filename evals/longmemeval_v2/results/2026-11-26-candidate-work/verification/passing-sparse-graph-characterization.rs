#[tokio::test]
async fn sparse_graph_read_has_bounded_planning_and_scan_work() {
    use memory_mcp::knowledge::KnowledgeGraphStore;
    let (_, db) = common::make_service_with_client().await;
    let store = KnowledgeGraphStore::new(db.clone(), "org");
    common::seed_entity(&db, "org", "entity:sparse_target", "person", "Target", &[]).await;
    common::seed_entity(&db, "org", "entity:unrelated_root", "person", "Other", &[]).await;
    common::seed_entity(
        &db,
        "org",
        "entity:unrelated_target",
        "person",
        "Other target",
        &[],
    )
    .await;
    let seeds = (0..32)
        .map(|n| format!("entity:sparse{n:03}"))
        .collect::<Vec<_>>();
    common::seed_entity(&db, "org", &seeds[0], "person", "Sparse", &[]).await;
    let payload = |id: &str| json!({"edge_id":id,"relation":"connects","origin":"inferred","strength":1.0,"confidence":1.0,"provenance":{"source":"test"},"t_valid":"2026-01-01T00:00:00Z","t_ingested":"2026-01-01T00:00:00Z"});
    store
        .relate_edge(
            "edge:visible",
            &seeds[0],
            "entity:sparse_target",
            payload("edge:visible"),
        )
        .await
        .unwrap();
    for n in 0..3000 {
        let id = format!("edge:unrelated{n:04}");
        store
            .relate_edge(
                &id,
                "entity:unrelated_root",
                "entity:unrelated_target",
                payload(&id),
            )
            .await
            .unwrap();
    }
    let reader = memory_mcp::knowledge::KnowledgeStoreClient::new(db, "org");
    ALLOCATIONS.with(|counter| counter.set(Some(0)));
    let rows = reader
        .select_edge_neighbors_for_entities(&seeds, "2026-05-01T00:00:00Z")
        .await
        .unwrap();
    let count = ALLOCATIONS.with(|counter| counter.replace(None).unwrap());
    assert_eq!(
        rows,
        vec![
            json!({"in":{"RecordId":{"table":"entity","key":"sparse000"}},"out":{"RecordId":{"table":"entity","key":"sparse_target"}},"relation":"connects"})
        ]
    );
    println!("SPARSE_GRAPH_ALLOCATIONS {count}");
    assert!(
        count < 100_000,
        "one sparse32-seed graph read must stay under100000 caller-thread allocations; observed {count}"
    );
}
