# Full-history V4 source-only cost review

The seven [V4 read receipts](reads/) and their retained stdout files under `/tmp/memory-quality-full-100-2026-11-27/observations-v4/` supplied the measurements below. The stdout SHA-256 matched each compact receipt. The comparison used the already retained [V3 receipts](../2026-11-27-full-history-restore/reads/). No reads or model calls were repeated.

## Fetched records and normalization

Each row describes one collected fact term-group pool, plus the separately observed episode pool. A multiplier indicates two separately observed fact pools with the listed volumes per pool. Bytes are logical UTF-8 lengths counted across projected records, including repeated content and keys. These counters do not measure the storage engine’s reads, network bytes, allocations or snapshot size.

| Case | Fact records | Fact body bytes | Index keys | Index key bytes | Exact content and index dictionaries per fact pool | Episode records | Episode body bytes |
|---|---:|---:|---:|---:|---:|---:|---:|
| `0c6b0c60` | 239 | 116,459,185 | 2,023,336 | 45,353,939 | 95 / 95 | 63 | 27,938,534 |
| `6d975a08` | 176 | 67,509,193 | 1,133,123 | 24,029,255 | 82 / 82 | 63 | 32,931,317 |
| `89a20f48` | 258 | 119,023,754 | 2,083,075 | 46,488,130 | 101 / 101 | 67 | 29,890,205 |
| `922a0c56` | 247 × 2 | 89,863,088 | 1,377,101 | 26,132,133 | 90 / 90 | 65 | 34,087,570 |
| `d6da3938` | 260 × 2 | 97,376,877 | 1,613,959 | 32,466,491 | 104 / 104 | 54 | 30,824,623 |
| `f43926d9` | 206 | 87,207,250 | 1,495,483 | 33,064,858 | 89 / 89 | 53 | 22,322,724 |
| `f61a096f` | 249 | 100,200,359 | 1,817,482 | 38,516,461 | 96 / 96 | 76 | 38,816,745 |

For `922a0c56` and `d6da3938`, the direct and alias-expanded branches each issued four grouped fact SQL statements for 30 distinct `MATCHES` literals. Each pair had 24 literals in common at the same positions, six that differed, and zero identical full batch SQL strings. The stage counters establish equal aggregate pool volumes. They cannot establish full equality of returned rows or exact key sequences. Admission separately built 298 index dictionaries across 151 records for `922a0c56` and two across one record for `d6da3938`. [Direct and expanded callers](../../../../crates/memory-mcp/src/memory/retrieval/pipeline.rs#L1716), [term grouping and late projection](../../../../crates/memory-mcp/src/knowledge/queries.rs#L206), [per-pool dictionary keys](../../../../crates/memory-mcp/src/memory/retrieval/lexical.rs#L504)

## Time measurements

All values below are milliseconds. Phase milestones are elapsed times since observation began. Completed SQL durations are measured at the client, may overlap and omit unfinished or failed operations. Child-process elapsed includes setup and teardown, measured apart from clone time. `—` means no value was observed.

| Case | Channels / fusion / admission / selection | Assembly | Child process | All completed SQL count / duration sum | Legacy heat SQL count / duration sum / max |
|---|---:|---:|---:|---:|---:|
| `0c6b0c60` | 9,967 / 12,950 / 15,590 / 17,608 | 19,502 | 33,155 | 86 / 8,814 | 5 / 3,594 / 1,623 |
| `6d975a08` | 9,054 / 12,188 / 14,495 / 16,912 | 19,553 | 31,273 | 79 / 10,274 | 5 / 5,727 / 1,834 |
| `89a20f48` | 9,968 / 13,366 / 15,744 / 17,940 | 18,413 | 31,055 | 84 / 6,589 | 4 / 1,181 / 304 |
| `922a0c56` | 14,616 / 17,797 / 20,109 / 23,064 | — | 45,118 | 182 / 10,494 | 0 / — / — |
| `d6da3938` | 15,840 / 19,001 / 20,233 / 22,085 | 22,718 | 38,633 | 153 / 11,298 | 1 / 514 / 514 |
| `f43926d9` | 9,853 / 13,258 / 15,560 / 17,436 | 19,573 | 32,643 | 89 / 11,942 | 4 / 6,761 / 1,887 |
| `f61a096f` | 10,702 / 14,345 / 16,607 / 19,289 | 24,697 | 37,349 | 117 / 22,281 | 5 / 15,491 / 3,535 |

V4 `922a0c56` reached selection before the external kill, with neither a final receipt nor a completed legacy heat update. The separate instrumented run observed the heat await start at 27,247.315 ms and a partial receipt at 31,627.812 ms without a settled phase or successful heat SQL. The two runs measure different events. [Heat diagnostic](../2026-11-27-access-heat-diagnostic/observation.json)

The separate native 37-case gate passed its registered quality floors in both versions. Wall time was 203.020 s in V4 and 262.009 s in V5 (+58.989 s). Reported case times were higher for 30 cases and lower for seven. The largest increase was 13.399 s for `locomo-conv-42-002`. These single-run values do not identify causal contributions from exact-key hydration, engine execution, scheduling or cache state. [Native descriptive comparison](../../../results/native-v5-registered-2026-11-27/descriptive-cost-vs-v4.json)

## Source delivery

Six completed V3/V4 reads had identical ordered content and quote bytes. V4 content and quote totals were each 10,240 bytes for `0c6b0c60`, `6d975a08`, `89a20f48` and `f61a096f`, 8,192 bytes for `f43926d9`, and 2,048 bytes for `d6da3938`. `922a0c56` delivered no context in either version and has no byte comparison. The strict registered span remained present in four of seven and independent source support in six of seven. [V4 decision](candidate-decision.json), [source review](source-support-review.json)

- `6d975a08` lost the full 86,868-byte carrier between admission and the bounded fact pool. The full episode fallback appeared among episode candidates but did not reach selection. Other delivered content independently supported the question. [Stage review](remaining-strict-span-stage-review.json), [candidate bound](../../../../crates/memory-mcp/src/memory/retrieval/pipeline.rs#L678)
- `d6da3938` selected the 804,658-byte carrier before packing. The same fact was delivered at 2,048 bytes without the strict registered span, while the requested relation remained supported. [Stage review](remaining-strict-span-stage-review.json), [packing boundary](../../../../crates/memory-mcp/src/memory/retrieval.rs#L196)

## Bounded follow-up checks

1. For the common terms across direct and alias-expanded reads, collect ordered per-term hit identities and scores with typed cutoff, type, limit and read-view identity. Check exact projected body and key identities separately. Reuse requires identical inputs and preservation of per-pass ordering, limits, first-hit union, native/alias provenance and delivered bytes. Differing term/order and differing cutoff are controls. The current observation supports profiling shared work while preserving the two distinct passes.
2. For native timing, measure channel reads, exact-key metadata read cardinality and client latency, metadata write await, and process overhead separately. Use a fixed alternating comparison on the same pinned subset and retain measured deltas and faults. Existing logs cannot assign the wall delta to one component.
3. Test rank bounding and packing independently. A synthetic rank fixture must reach admission before testing whether conjunctive source evidence survives the existing candidate cap. An independent excerpt fixture must prove contiguous source bytes and offsets under the existing 2,048-byte cap. Keep the original strict labels and independent source review for any corpus replay.
