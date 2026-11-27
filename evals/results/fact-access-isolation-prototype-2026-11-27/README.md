# Fact-access storage-isolation prototype — 2026-11-27

**Registered prototype gate: PASSED. Production and full-history acceptance are separate.** The experiment measures a temporary-schema storage alternative on native Apple Silicon with lockfile-pinned SurrealDB 4.0.1 / SurrealKV. It does not measure end-to-end recall or prove an FTS-rebuild cause.

The immutable [registration](registration.json), [registered specification](spec.registered.md), [archived test](fact_access_payload.at-registration.rs), [archived production writer](fact_access_store.at-registration.rs), [runner](runner.at-registration.py), [preflight](preflight.log), [raw timed log](prototype.log) and [observation](observation.json) describe the executed version. The test binary was built with **`CARGO_PROFILE_TEST_OPT_LEVEL=3`**, not inferred from a later production build. Registration pins its binary, source, lockfile and preflight hashes.

Two fixed cases use a 27-byte body with 1 or 8,192 index keys. Each has 20 alternating AB/BA pairs, with a separately timed first-access arm after every pair: **2 cases × 20 pairs × 3 arms = 120 attempted / 120 completed timed observations**. Each order stratum contains ten pairs per key count. There was one timed dispatch, no test-run retries, no runtime fault and exit code 0; captured wall time was 11.452 seconds. Preflight and reopen contract checks passed. Failures would have remained in the capture under the registered stop policy.

The arms differ deliberately:

- **Legacy:** the then-current production fact-row writer, one awaited update with `RETURN NONE`.
- **Isolated:** a test-only exact-key side-stat read followed by a narrow existing-side-row update, measured together as two logical queries.
- **First access:** a separately timed test-only existence/create-or-increment transaction, one logical query; verification/reset is outside its timer.

The later production side-table transaction is not identical to the isolated two-query prototype arm. Its correctness and actual workload cost require separate integration and full-history checks. Logical query counts and SQL/JSON request/response bytes are API-boundary measurements, not engine KV bytes or engine statement counts.

| Keys / order stratum | Legacy median ms | Isolated combined median ms | Isolated / legacy | First-access median ms |
| --- | ---: | ---: | ---: | ---: |
| 1 / AB | 5.232 | 4.748 | 0.908 | 6.217 |
| 1 / BA | 4.787 | 5.138 | 1.073 | 6.084 |
| 8,192 / AB | 167.106 | 4.935 | 0.0295 | 5.963 |
| 8,192 / BA | 166.402 | 4.431 | 0.0266 | 6.092 |

The registered high-key gate requires the ratio to be at most 0.50 in **both** order strata; both pass. The low-key BA isolated median is 7.3% slower than legacy, while AB is faster. First access is slower than legacy in both low-key strata. This single capture neither establishes a repeatable low-key regression nor justifies claiming universal speedup or no cost tradeoff.

The result supports reviewing storage isolation for high-key facts. It does not separate full-row encoding, index-value comparisons, KV writes, cache effects or other engine costs, and provides no evidence that an FTS index was rebuilt. Migration safety, production ranking/archival/decay parity, the unchanged full-history deadlines and delivered-evidence assertions, reader qualification and independent memory-utility confirmation remain outside this prototype gate.
