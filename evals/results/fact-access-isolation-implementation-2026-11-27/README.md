# Fact access isolation: implementation evidence

User-approved migration057 separates mutable access statistics from large immutable facts. Prototype admission preceded approval and production implementation. See the [approved specification](../../../docs/superpowers/specs/2026-11-27-fact-access-storage-isolation-design.md) and [operator cutover/rollback runbook](../../../docs/operations/FACT_ACCESS_UPGRADE.md).

## Contracts checked

- Exact signed/saturating counters, timestamps, missing-fact behavior, source identity, concurrent first access and serial repeated identities; heat-only writes leave fact content, quote, keys and vectors unchanged.
- Bounded exact-key heat reads before fusion/ranking; mutable heat remains absent from revision-bounded history. Decay reads stats before any invalidation and aborts on a read error; archival preserves its inclusive cutoff and zero-count/recent-access rules.
- Migration057 uses atomic128-row keyset pages and cursor, exact replay/reconciliation and owner-fenced terminal state. Operational writer fencing is required: migration ownership is not a fence for predecessor processes.
- Native SurrealKV upgrade, close/reopen, verified backup/restore and reverse transfer3/3. This is embedded native fixture evidence, not an actual predecessor-binary or remote-engine rollout drill.

[Source review v2](source-review-v2.json), [successful consolidated verification](../memory-quality-verification-2026-11-27/storage-isolation-final.json), and [native upgrade tests](../../../crates/memory-mcp/tests/fact_access_native_upgrade.rs) retain exact limits and proof boundaries. Earlier failed verification and test-close attempts remain archived. An initial nondiscriminating decay fake is excluded; the corrected delegated fault adapter failed against the predecessor behavior and passed against the new preflight.

The successful production check counted2270 tests and12 ignored; harness571 passed and3 ignored. Later equivalent slice assertions were checked6/6, not counted again as additional overall coverage. Mandatory workspace clippy, eval-support clippy and fmt passed. Python checks and optional SDK controls are linked in the consolidated receipt. Source changes after that broad run are the separate narrow UI-query rule and its scoped controls below.

## Full-history acceptance remains distinct

[V5 paired capture](../../longmemeval_v2/results/2026-11-27-full-history-access-isolation/README.md): both arms completed7/7 under the original30/45s, with equal delivered items7/7. Strict spans4/7 and independently sufficient sources6/7 failed the combined7-source gate. Storage/native contract checks passed; this did not make V5 qualified. The later separate [V7 source-selection replay](../../longmemeval_v2/results/2026-11-27-full-history-positional-v7/README.md) passed7/7 source sufficiency and5/7 strict spans; this is exposed source qualification, not reader/agent benefit. The fresh legacy922 also completed, so no paired6→7 runtime claim is warranted. One pair per exposed case does not establish a general speed effect.

## Independent UI-selection repair

[Positional-control TDD](positional-controls-tdd.json) records two genuine public REDs: a carrier admitted at rank25 was lost before candidate bounding; the first grammar extension also incorrectly prioritized quoted team names in an unrelated dispute. The final rule binds `between` to a nearby UI-control noun and preserves the existing membership, negation and unrelated-query controls. Stage integration50/50 and query-mode7/7 passed. No gold IDs, expanded budgets or generic temporal grammar were introduced.

[V7 checks](positional-v7-checks.json) and [binary pins](binary-pins-v7.json) are distinct from the [superseded build-only V6](binary-pins-v6.json). V6 produced no native/full-history/reader evaluation outputs; the independent dispute RED was fixed before dispatch. Exact dispatch scripts are archived with [SHA receipts](v7-dispatch-sources.json). V7 native/full-history outputs are retained separately under their registered directories and summarized in the [main report](../../../docs/evals/MEMORY_QUALITY_FOLLOWUP_2026-11-27.md).

No deployment, commit or push is part of this delivery. Preserve the closed original and upgraded initial snapshots and pinned executables until user acceptance.

Final read-only review found a concrete incompatible-index readiness false pass. [Genuine public RED/GREEN](fact-access-index-readiness-tdd.json) proves rejection of preexisting UNIQUE while12 upgrade/native integrations and5 field-readiness controls pass. The fact_access-only canonical comparison also rejects extra fields and alternate kinds; no other table policy changed. Final mandatory workspace/eval clippy and fmt passed in [post-guard checks](positional-final-readiness-checks.json). V7 evaluation receipts precede this guard and remain labelled accordingly.
