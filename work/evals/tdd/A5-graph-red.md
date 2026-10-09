# A5 RED evidence — graph admission and storage-read batching

- rtk cargo test -p memory_mcp --locked --lib relationship_intent_accepts_graph_evidence_with_known_anchor_origin — exit 101. The relationship-intent candidate with a known query-entity origin and two-hop trace was rejected as InsufficientEvidence instead of admitted as GraphRelationship.
- rtk cargo test -p memory_mcp --locked --lib graph_neighbor_reads_batch_seed_nodes_into_one_store_call — exit 101 after correcting the fixture to valid Surreal record IDs. The real embedded store returned the target trace, but traversal issued 6 edge queries for 3 seed nodes; the intended contract is one batch read.

An earlier RED attempt used hyphens in Surreal record keys; the database parsed those as subtraction, so that fixture failure was discarded and is not counted as product evidence.
