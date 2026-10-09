# B2 historical assembly projection — RED

Base revision: `355dc92206e341d02a61a6066cd7ec5e49f21ce0`

The failure-first case uses two real ingested episodes, the production claim extraction and reconciliation pipeline, and the actual `assemble_context` tool. After the persisted supersession is observed, the test writes and reads back a transaction-time invalidation later than the requested `as_of`, then asks for the historical pack.

```text
rtk cargo test -p memory_mcp --locked --features test-fixtures --test assemble_context_reconciliation exposes_supersession_on_the_predecessor_item -- --exact --test-threads=1
exit 101
```

The existing active-only relation reader omitted the supersession from a pack whose `as_of` precedes its persisted invalidation. The test failed on the observable historical-pack assertion; ingestion and invalidation readback succeeded.

Evidence hashes:

- Retrieval source at RED: `480400fab0e9cddd458db2f8cc9b7d454f8608e498f3fcf54769f2bf741b2e28`
- Integration test source at RED: `06084c7ea318cbd3166cf2b9782d9632ddb199197d277902917a13f305e1d762`
- Test output: `87e1a849d6ab2f874f211b1a89ad2590a014f79ea75043f2f87fcb54792f8e23`
