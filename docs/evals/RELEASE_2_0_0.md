# Release 2.0.0 qualification

> Hosted release CI subsequently failed before packaging; v2.0.0 has no binary
> assets. See [the 2.0.1 recovery report](RELEASE_2_0_1.md). Local results below
> remain valid within their stated scope.

## Native evaluation

The final frozen native executable passed **154/154 cases**, all **12 gates**,
with **0 invalid executions**. The retrieval-quality subset passed **37/37**
and all six unchanged 1.0 acceptance floors. The run took 272.621 seconds;
this is a descriptive duration under concurrent host load, not a speed claim.

[Final raw report](../../evals/corpora/retrieval-quality-v1/native-quality-r28-release-final.json)
and [preparation and regression receipt](../../evals/corpora/retrieval-quality-v1/native-preparation-refresh-r28.json)
bind sources, labels, runtime configuration, model identities, executable and
source hashes. The original 36/37 native failure and intermediate 153/154 release
failure remain alongside the final result. No case, acceptance floor or denominator
was relaxed.

The repairs align the harness with the production claim configuration, distinguish
terminal project scope from exact content identifiers, and preserve already verified
secondary-facet proof through final reranking. Independent public controls cover
other-project sources, mixed sources, explicit targets and negative reranker scores.
Weak words spread across incomplete facets still fail admission.

## Local release checks

[Local preflight receipt](../../evals/results/release-2.0.0/local-preflight.json)
records commands, outcomes and log hashes. Coverage includes the workspace suite,
the production feature suite, mandatory workspace clippy, UI WASM clippy,
observability/toolchain/Dioxus checks, disabled filesystem ingestion and formatting.
Additional scoped checks cover blocked GLiNER refresh, response-size budgets and
performance smoke. Architecture lints and intentionally ignored tests are not
counted as production scenario coverage.

The workspace suite precedes the final one-line facet-proof repair; the feature
suite, scoped semantic/reranking controls and native release profile cover that
repair. The version bump changes package metadata only. The post-bump public
binary gate passed **8/8**, with zero failed, invalid or unobserved cases,
`no_regressions=true` and `all_cases_pass=true`. Its
[receipt](../../evals/results/release-2.0.0/public-gate-receipt.json) and
[raw comparison](../../evals/results/release-2.0.0/public-gate/current-regression.json)
were archived after publication. An immutable executable copy prevented concurrent
Cargo builds from replacing the input; its SHA was unchanged after capture.

All local evaluation runs use native Apple Silicon. Local checks do not establish
remote SurrealDB, Docker or other-target compatibility. Hosted release CI is a
separate publication check; its status must be read from GitHub.

## What these results do not establish

These are exposed development regressions and finite retrieval contracts, not an
independent accuracy estimate, an official full benchmark score or proof of improved
agent outcomes. The [earlier reader report](MEMORY_QUALITY_REPAIRS_2026-11-26.md)
remains historical evidence: 4/7 answer correctness and 0/3 false-premise rejection
were measured on its own pinned build. Those reader results were not rerun or
relabelled for this release. C2 remains NO-GO until workflow qualification and a
new preregistered independent study succeed.

Remaining work includes inference-attempt accounting, reader refusal/escaping,
the unresolved long-history case, durable access-update cost, independent entity
quality, citation usability and behavioural skill evaluation. The detailed [post-release implementation plan](../superpowers/plans/2026-11-26-post-release-memory-quality.md)
was added after publication, separately from the immutable release tag.
