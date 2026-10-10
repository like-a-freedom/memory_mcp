# C41 timeline order with persisted supersession

The integration test creates two source-lineage episodes through production ingest, waits for reconciliation to persist their supersession, and assembles a timeline through the public capability. It asserts both facts remain present, the predecessor points to the actual successor, and event order stays chronological.

RED: `rtk proxy cargo test -p memory_mcp --locked --test assemble_context_supersession_ranking timeline_keeps_persisted_predecessor_before_its_successor -- --exact --nocapture` failed because unconditional post-assembly supersession demotion put the later successor first.

GREEN: the same embedded-store test passed after timeline output retained reconciliation metadata and bypassed only the supersession reorder.
