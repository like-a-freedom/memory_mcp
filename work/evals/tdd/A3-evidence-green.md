# A3 structured evidence GREEN

Admission now supports two explicit, inspectable evidence paths in addition to direct lexical/reference/entity/semantic evidence:

- A community candidate requires sufficient query-term coverage in a matched persisted summary and an exact link from the fact to the matched member entity. A weak direct result remains eligible for this path and ranking merges the same fact identity without duplicating the response.
- An experience candidate requires a query-anchor index key plus a repeated topic supported by at least two distinct query-grounded direct facts. Direct candidate retrieval has a named floor of five while the requested budget still caps the returned context.

The selected basis and matched supporting terms/IDs are serialized under `provenance.retrieval_admission`. A graph-only candidate on a content query still fails unless its own lexical/semantic evidence passes.

Validation:

- Admission policy: 25 passed, including positive and negative community/repeated-topic boundaries.
- Repeated-topic derivation: 2 passed, including the exact persisted hotel/conference scenario and a no-query-anchor negative.
- Real community integration: 1 passed against production migrations and persisted embedded SurrealDB community/fact rows.
- Existing community ordering and extracted-vs-inferred ranking integration tests: 2 passed.
- Real repeated-topic integration case: 1 passed with two source facts, a budget of one, and provenance readback naming both supporting IDs.
- Weak one-term graph expansion content query: 1 passed with only the directly grounded result retained.
- Full `memory_mcp` library suite: 1,760 passed, 1 ignored.

These are component and embedded-store checks, not corpus-backed eval acceptance. Reviewed labels, provider/model/index fingerprints and G0/G5 remain open.
