# A5 GREEN evidence — provenance-qualified graph retrieval

- rtk cargo test -p memory_mcp --locked --lib memory::retrieval::admission::tests — PASS, 20 tests, including stable first-32 admitted seeds, graph-only rejection for content intent, missing-origin rejection, and relationship admission.
- rtk cargo test -p memory_mcp --locked --lib memory::retrieval::graph::tests — PASS, 3 tests against real embedded SurrealDB. Covers one-hop query-entity origin, admitted-seed origin, required second hop, third-hop exclusion, and one batched edge query for three seeds.
- rtk cargo test -p memory_mcp --locked --lib ranked_graph_candidate_carries_anchor_provenance_into_admission — PASS; ranking carries origin/hop evidence to the policy decision.
- rtk cargo test -p memory_mcp --locked --lib canonical_fact_replacement_preserves_graph_anchor_evidence — PASS; duplicate canonical replacement does not erase graph evidence.

This closes component behavior only. Native L19–L23 corpus acceptance remains unavailable until the reviewed corpus and pinned provider/index are prepared.
