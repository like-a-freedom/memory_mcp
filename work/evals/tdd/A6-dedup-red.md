# A6 RED evidence — persisted Duplicate view deduplication

The first compile attempt exposed a test-only missing `DbClient` trait import. That run is not counted as behavioral RED. After adding the import, the isolated integration test compiled and created a real Duplicate relation through production episode ingestion, extraction, claim projection, and reconciliation.

Command:

```text
rtk cargo test -p memory_mcp --locked --features test-fixtures --test assemble_context_reconciliation deduplicates_identical_facts_only_after_persisted_duplicate_relation -- --exact --test-threads=1
```

Result: exit 101 on the behavioral assertion. The fixture verified a persisted active `duplicate` relation, then `assemble_context("ARR")` returned 2 identical facts where the contract expects 1 representative. The failure is in view dedup, after realistic persisted relation setup; it is not a fixture, database, or compilation failure.

RED log SHA-256: `3cf6f68ec570ccec58d86afa177e971901c3872c94c7802299f0a57e83c5a0c7` (`/Users/solovey/Library/Application Support/rtk/tee/1791501251_cargo_test.log`).

Source blob hashes captured at RED:

- `memory/retrieval/ranking.rs`: `2bd950d456b0040837ecd9886c84247f93276932`
- `memory/retrieval.rs`: `49c063ae67c00f7f001c258ebbda97b5d48c9776`
- `knowledge/claims.rs`: `76fc40e995e9e8169ff819f23df42971d67189a0`
- `knowledge/api.rs`: `d099fd782ace44f522b6aa841a609aff2567e6f0`
- `tests/assemble_context_reconciliation.rs`: `5c2167d76cdabdec5c783bcf758135247b26256c`

The C04 access-control regression also has a valid real-store RED:

```text
rtk cargo test -p memory_mcp --locked --features test-fixtures --test explain_provenance explain_returns_all_allowed_lineage_without_restricted_sources -- --exact --test-threads=1
```

The test ingested three source episodes and facts with production capabilities, linked them to the same persisted entity, and called `explain` with `allowed_tags=[visible]`. It returned all three episodes, including the `restricted` source, where only the two `visible` source episodes are expected. Exit 101 is the source-leak assertion, not setup failure. RED log SHA-256: `387b59cb08e7ee083e289f5a1d46fad11aa84587947f1511d0d30f742193d0b5` (`/Users/solovey/Library/Application Support/rtk/tee/1791501821_cargo_test.log`).

Source blob hashes at this RED:

- `memory/explanation.rs`: `2dad58d174bc30814db07b44c9a048d4df767e15`
- `memory/episode_context_store.rs`: `b7cbe607ab81627d05581074faa6be9346c77509`
- `tests/explain_provenance.rs`: `c6b92b051eeef6db026dbd23026bb6683e8ddff0`
