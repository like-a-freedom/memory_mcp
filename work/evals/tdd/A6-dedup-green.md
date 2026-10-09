# A6 GREEN evidence — visible Duplicate view dedup and provenance access

The view now collapses only exact normalized content connected by a persisted `Duplicate` relation returned by the evidence-stage relation reader. Relation reads resolve participant fact and episode IDs only from the requested visible fact set; an unselected counterpart remains unnamed. The highest-ranked item stays the representative, relation summaries are retained by relation ID, and the assembly is cached after dedup. Retrieval does not create or invalidate claim relations.

`explain` now applies the request's `allowed_tags` to the direct episode, direct fact, and entity-linked episode/fact read. It collects links only from an accessible fact, filters linked episodes by both episode and fact tags in the owner query, and clears citation metadata if the direct episode is absent or inaccessible.

Validation:

- `rtk cargo test -p memory_mcp --locked --lib ranking::tests` — 35 passed. Covers same normalized content with a visible Duplicate group, identical content without a visible group, and differing content inside a group.
- `rtk cargo test -p memory_mcp --locked --features test-fixtures --test assemble_context_reconciliation -- --test-threads=1` — 6 passed. The positive case uses real ingestion, claim projection, and persisted relation readback; the relation count is unchanged across assembly; a budget-one case has no counterpart episode in public metadata; supersession, rollout gating, and reorder behavior remain covered.
- `rtk cargo test -p memory_mcp --locked --features test-fixtures --test explain_provenance -- --test-threads=1` — 12 passed. The C04 case proves both allowed linked episodes remain available while denied fact and denied episode policies each exclude their source. Existing direct, linked, unknown-record, and access-tracking cases pass.

The existing supersession fixture asked for `ARR legacy`, which the A3 evidence floor correctly reduced to the directly grounded predecessor because the successor had only one of the two query anchors. The test now asks for `ARR`, which directly matches both revisions and isolates the supersession ordering contract. This does not claim that a relation alone expands a content query.

These are production-path and embedded-store checks, not reviewed-corpus eval acceptance. G0/G5 and the native provider/index fingerprints remain open.
