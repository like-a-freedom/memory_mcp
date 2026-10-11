#![cfg(feature = "eval-support")]

mod common;

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use chrono::Utc;
use memory_mcp::eval_support::{QueryObserver, SqlObservation};
use memory_mcp::models::IngestRequest;
use memory_mcp::service::memory_container_shims::memory_capabilities_extract::ExtractCapability;
use memory_mcp::service::memory_container_shims::memory_capabilities_ingest::IngestCapability;

/// Counts the neighbor-lookup SELECTs the community walk issues against the
/// edge table. The full-table edge scan uses a different SELECT shape, so this
/// counter stays specific to the per-node neighborhood queries.
#[derive(Default)]
struct EdgeNeighborCounter {
    selects: AtomicUsize,
}

impl QueryObserver for EdgeNeighborCounter {
    fn observe(&self, obs: &SqlObservation) {
        if obs.sql.contains("SELECT in, out, relation FROM edge") {
            self.selects.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// A burst of extracts touching the same entities must not re-walk the whole
/// historical component each time. The defect was a fresh BFS per extract with
/// two edge-neighbor SELECTs per component node, so statement count grew
/// quadratically with corpus size (97% of SQL time at scale 100). The fix may
/// issue a constant number of neighbor lookups per extract — seeded from the
/// already-stored community membership — never a count that scales with the
/// accumulated component.
#[tokio::test]
async fn community_burst_keeps_edge_neighbor_selects_bounded_per_extract() {
    const BURST_EXTRACTS: usize = 8;
    const MAX_EDGE_NEIGHBOR_SELECTS_PER_EXTRACT: usize = 16;

    let (service, db_client) = common::make_service_with_client().await;
    let counter = Arc::new(EdgeNeighborCounter::default());
    db_client
        .set_eval_query_observer(Arc::clone(&counter) as Arc<dyn QueryObserver>);

    let mut last_extract_selects = 0usize;
    for i in 0..BURST_EXTRACTS {
        let episode_id = IngestCapability::ingest_from_service(
            &service,
            IngestRequest {
                source_type: "email".to_string(),
                source_id: format!("MSG-BURST-{i}"),
                content: "ARR grew to $3M. I will send the update by Friday.".to_string(),
                t_ref: Utc::now() - chrono::Duration::days(1),
                t_ingested: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .expect("ingest");

        let before = counter.selects.load(Ordering::SeqCst);
        let extraction = ExtractCapability::extract_from_service(&service, &episode_id, None, None)
            .await
            .expect("extract");
        last_extract_selects = counter.selects.load(Ordering::SeqCst) - before;

        assert!(
            extraction.entities.len() >= 2,
            "fixture must extract at least two entities so community updates run, got {}",
            extraction.entities.len()
        );
        assert!(
            last_extract_selects <= MAX_EDGE_NEIGHBOR_SELECTS_PER_EXTRACT,
            "extract {i} issued {last_extract_selects} edge-neighbor selects; \
             the community walk must stay incremental, not re-walk the growing component"
        );
    }

    assert!(
        last_extract_selects > 0,
        "community walk must still issue edge-neighbor lookups for fresh nodes"
    );
}
