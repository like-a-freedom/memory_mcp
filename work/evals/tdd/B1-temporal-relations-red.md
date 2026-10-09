# B1 temporal relation reads — RED

Base revision: `355dc92206e341d02a61a6066cd7ec5e49f21ce0`

The first failure-first run compiled the new storage contract tests against the current implementation:

```text
rtk cargo test -p memory_mcp --locked --lib temporal_relation_reads -- --nocapture
exit 101
```

It failed at compile time for the expected product gap: `TemporalRelationsQuery` and `SurrealClaimStore::select_temporal_relations` did not exist (six E0422/E0599 errors). This is an API-contract RED; no runtime claim is made from it.

Evidence hashes:

- Test/source blob at RED: `c6c9bf1cb705c44afff056b0fa24eaff9e0af3b183f77795a1daf26841810e7c` (`crates/memory-mcp/src/knowledge/claims.rs`)
- Compiler output: `8304fb43e27f637dd83c7916f02346043f8746ad9e04dced4dea1436df37855d`

Required GREEN behavior from these tests:

- A relation is visible before its `t_invalid_ingested` time and absent afterward at the requested cutoff.
- Counterpart facts are batch-hydrated only when claim and fact policy tags permit them; denied fact IDs/content are omitted from hydrated results and directional metadata.
- The result retains an internal unresolved marker so a denied counterpart is distinguishable from a confirmed absence without publishing its identifier in the tool response.
