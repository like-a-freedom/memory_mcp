# Durable access characterization

One registered local Apple Silicon capture, isolated real embedded stores, serial test order. Four payload/key combinations: 27 or 270000 body bytes, 1 or 8192 keys (18 or 170922 key bytes). Each focal call performs exactly one awaited write, no whole-record read, 554 request bytes and 2 response bytes. Persisted increment, timestamp, immutable body and keys are checked.

Observed update times: small body/small keys 1.386 ms; large body/small keys 1.944 ms; small body/large keys 162.323 ms; large body/large keys 164.564 ms. Single observations are descriptive, not an effect estimate, engine-cause proof or RSS measurement. Large key arrays warrant an engine/write-amplification investigation. This is not a comparison against the removed full snapshot.

The current application statement already atomically changes only heat fields and returns NONE. There is no observed redundant application read, payload return or repeated write to remove. No speculative production rewrite was made. Moving heat to a side record changes schema, migration/backfill, fact projections, decay and durability contracts; it needs a separately reviewed design and the repository's migration approval. Fire-and-forget would weaken the contract.

Four wave/failure/cancellation/durability integration controls and five payload/resource checks pass. Existing owner integration checks additionally cover arithmetic bounds, absent records and vector-write ordering. Reproduce with `cargo test -p memory_mcp --features eval-support --test fact_access_payload --test fact_access_concurrency --locked -- --nocapture --test-threads=1`. This command records fresh timings rather than asserting a noisy wall-time threshold.
