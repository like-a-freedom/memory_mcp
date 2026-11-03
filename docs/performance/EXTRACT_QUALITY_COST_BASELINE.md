# Extract quality and cost baseline

Status: local implementation verified, 2026-11-03. The production figures are a
historical baseline; controlled tables compare the implemented policy with
pristine `8d148440`. No production rollout or historical repair was performed.

Production: Grafana context `grafana-pushkino`, VictoriaMetrics datasource
`edr4onptd6hogd`, `job="memory_mcp"`, evaluation time
`2026-11-02T10:55:00Z`. The following are Prometheus `increase` estimates;
the 24-hour population is small and counters can reset.

| Observation | Window | Value |
| --- | --- | ---: |
| Extract calls | 24 h | 7 success, 2 error |
| Mean extract duration, all outcomes | 24 h | 19.51 s |
| Mean extract duration, all outcomes | preceding six days | 3.02 s |
| Extract results | 24 h | 378 facts, 377 entities, 30,116 warnings |
| Extraction stage | 24 h | 4 observations, 30.38 s mean |
| Attribute reconcile candidate count | 24 h | 82.35 mean per observation |
| Attribute reconcile duration | 24 h | 40.06 s total |

Queries at the fixed evaluation time:

```promql
sum by (operation,outcome) (increase(memory_operation_duration_seconds_count{job="memory_mcp",operation="extract"}[24h]))
sum(increase(memory_operation_duration_seconds_sum{job="memory_mcp",operation="extract"}[24h])) / sum(increase(memory_operation_duration_seconds_count{job="memory_mcp",operation="extract"}[24h]))
sum by (operation,result) (increase(memory_operation_results_total{job="memory_mcp"}[24h]))
sum by (operation,stage) (increase(memory_pipeline_stage_duration_seconds_sum{job="memory_mcp"}[24h])) / sum by (operation,stage) (increase(memory_pipeline_stage_duration_seconds_count{job="memory_mcp"}[24h]))
sum by (schema,match_mode) (increase(memory_claim_candidates_considered_sum{job="memory_mcp"}[24h])) / sum by (schema,match_mode) (increase(memory_claim_candidates_considered_count{job="memory_mcp"}[24h]))
sum by (stage,schema,outcome) (increase(memory_claim_pipeline_duration_seconds_sum{job="memory_mcp"}[24h]))
sum by (outcome) (increase(memory_claim_pipeline_total{job="memory_mcp",stage="reconcile"}[24h]))
```

The 30,116 warnings and 30,116 contradiction outcomes are aggregate counts;
their equality does not establish a per-request causal link. Four stage
observations and nine operations are different populations, so their averages
are not additive. Candidate counts include the candidate page, and do not
prove how many comparisons completed within an inline budget. Response size,
input size and per-request breakdown are not established by these queries.

The real-storage regression `unscoped_historical_records_do_not_form_comparable_claims`
failed before the subject gate: both independent historical records produced
claims. A separate linked-subject case retained a genuine contradiction after
the experimental gate. Controlled corpus profiling and comparisons are reported below.

## Controlled historical-record workload

The ignored `profile_extract_quality_corpus` integration profile constructs a
fresh embedded database per size. Each episode has a `Facts:` section with 5,
20, or 50 synthetically generated `Historical record: {JSON}` lines; the
large episode is then extracted a second time. The database is initially empty;
it accumulates the earlier lines of the same episode during extraction. Wall
time below includes ingest plus extract for the first call, and extract alone
for repeat. Results are single debug-build observations, not latency percentiles.

| Case | Input bytes | Baseline ms | Current ms | Baseline warnings | Current warnings | Baseline response bytes | Current response bytes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | ---: |
| small | 287 | 32 | 16 | 6 | 0 | 2,800 | 576 |
| medium | 1,138 | 119 | 21 | 154 | 0 | 54,204 | 1,446 |
| large | 2,848 | 518 | 49 | 961 | 0 | 331,110 | 3,186 |
| repeat | 2,848 | 203 | 19 | 961 | 0 | 331,110 | 3,186 |
| 20 pre-existing conflicts | 32 | 16 | 12 | 20 | 20 | 6,445 | 6,445 |

Both builds returned 5, 20, 50 and 50 facts respectively. Baseline returned
2/3/4/4 entities; the current build returned 1/1/1/1. Baseline was a Git
archive of `8d148440` with only the profiling test copied into it. The two
builds shared the Cargo target directory; each was explicitly rebuilt and the
baseline binary was checked against the failing date/percentage regression to
avoid reusing the changed library. Command, from each respective checkout:

```sh
cargo test -p memory_mcp --test extract_quality_e2e profile_extract_quality_corpus --locked -- --ignored --nocapture
```

The last row seeds 20 facts under one resolved entity and one attribute, then
extracts a different value. It verifies that this small genuine-conflict case
still returns all 20 warnings. This synthetic case shows the cost of false
warning amplification and the effect of removing those false comparisons; it
does not identify the production hotspot or the cost of *many* genuine
contradictions. A representative production database and production per-request attribution
remain unmeasured. The additional controlled scale and stage results below delimit
what this optimization establishes.

The real-store `warning_materialization_reuses_the_current_fact_for_all_relations`
test counts individual record reads for 20 conflicts: the new fact was read 22
times before the warning-content cache and at most 3 times afterwards (two
other extraction stages still read it). All 20 warning bodies remain present.
This removes duplicate reads within a request; it is not a batch query and does
not bound the response size. Failed or missing reads are retried as before.

## Genuine-conflict scaling and stage attribution

The separately ignored `profile_genuine_conflict_scale` seeds 20/50/100
same-slot facts, extracts a new distinct value and asserts that all counterpart
warnings remain present. Both builds used the identical fixture.

| Existing facts | Baseline ms | Current ms | Warnings, both | Response bytes, both |
| ---: | ---: | ---: | ---: | ---: |
| 20 | 23 | 12 | 20 | 6,487 |
| 50 | 45 | 31 | 50 | 15,157 |
| 100 | 66 | 48 | 100 | 29,607 |

The two ignored profiles ran sequentially in one process: baseline 4.04 s
wall, 3.48 s user CPU and 0.22 s system CPU; current 2.518 s wall, 2.352 s user
and 0.078 s system. Setup/seeding is included in process figures. Current
process peak RSS from `getrusage(RUSAGE_CHILDREN)` was 121,716,736 bytes
(about 116 MiB). Baseline peak memory was unavailable because macOS denied
`time -l` its sysctl request; no memory improvement is claimed.

`profile_existing_stage_metrics` installs the existing Prometheus recorder
and subtracts pre-extract samples from post-extract samples. Run it separately
with `--features prometheus --test extract_quality_e2e
profile_existing_stage_metrics -- --ignored --nocapture`. It adds no production
instrumentation. On 50 unrelated JSON records, baseline attribute reconciliation
performed 50 observations / 1,011 fetched candidates and took 178.55 ms;
current produced no claims, candidates or reconciliation work and no warnings.
On 100 genuine counterparts, baseline attribute reconciliation fetched 101
candidates in one observation / 17.28 ms; current quantity reconciliation
fetched 101 / 21.37 ms. Counts include the new claim itself. These observations
locate avoidable false-comparison work; they do not show a candidate-query
speedup. The cache regression independently proves fewer warning-body reads.
NER, serialization and remaining extract work are outside that claim-stage
histogram and cannot be obtained by adding unrelated production means.

Artifacts in this local session are `/private/tmp/extract-quality-baseline-profile-final.log`,
`/private/tmp/extract-quality-current-profile-final.log`,
`/private/tmp/extract-quality-stage-baseline.log` and
`/private/tmp/extract-quality-stage-metrics.log`. Single debug-build runs are
sensitive to host scheduling; production percentiles require a deployed cohort.

## Quality acceptance and semantic contract

The fixed PR profile runs the unchanged extraction (9 cases) and
claim-reconciliation (41 cases) suites. The final artifact is
`/private/tmp/extract-quality-current-eval-final-v3.json`; the baseline artifact
is `/private/tmp/extract-quality-baseline-eval.json`. Both pass 50/50, with
claim precision and recall 1.0. Entity precision/recall remain 0.6/1.0 on the
nine extraction cases. These are small synthetic corpora, not population
quality estimates. An intermediate fallback introduced an extra false entity
in ext-001 by spanning a sentence boundary; the new regression caught it and
the final evaluation restores the baseline entity metrics without fixture edits.

Intermediate subject and colon gates failed quality checks (39/50 and then
49/50) and were replaced. The final policy is:

- Generic free-text attributes cannot declare single-valued cardinality.
  Compatible descriptions (runner and coach) must not contradict one another.
  The original fact remains stored and retrievable even without a claim.
- Complete explicit schema records with a declared `subject` (`dimension` + `value`, `measure` +
  `value`/optional `unit`, `predicate` + `object`, `action` + `target`) are
  recognized through the existing schema contract. Fields use newline or
  semicolon separators. Unknown or duplicate keys do not qualify. A complete
  record produces one sourced fact, rather than several heuristic fact kinds. Colon and equals headings alone do not qualify.
- Supported typed quantities and commitments retain comparisons. Dollar
  quantities in supported reporting constructions are normalized exactly using
  decimal arithmetic, including K/M scales. No float comparison is introduced.
- Exact subject phrases bind through resolved entity names. Nested reporting
  and compound/ambiguous subject phrases do not borrow the first person's ID.
  Several independent copular assertions are not bound to one subject.
  A subject cannot span sentence punctuation; the mixed metric/promise regression
  prevents an entire preceding sentence from becoming a synthetic entity.
  Discourse pronouns and coordinated subject phrases do not create a new
  grammatical identity. The two additional real-store regressions failed
  before that eligibility rule and pass after it.
- The supported multilingual quantity grammar contributes a neutral `concept`
  mention if NER omitted its subject. It uses the existing entity resolver and
  does not pretend to infer a person type. The Chinese case now passes without
  reinstating the empty-subject bucket.
- Names already fetched for fact indexing are reused by projection. There is
  no additional post-commit name read. A preparation failure occurs before fact
  creation; an integration fault/retry test proves the episode can be retried
  and claims are created successfully.

This policy follows the distinction between a property and an explicitly
functional property: multiple values do not alone establish inconsistency.
See the [W3C OWL 2 Primer](https://www.w3.org/TR/owl2-primer/). This is a design
rationale, not a claim that this service implements OWL reasoning.

A real-store adversarial test exposed a subject-binding defect in
`Alice Smith says Bob Jones reports ARR is $10M.`. Warning-only assertions
missed the incorrect persisted subject. The regression now inspects stored
subject identity and passes. Repeated identical reporting markers are also
covered.

New claim projections use `builtin-comparable` policy version 2; entity
projection tokens append `candidate-policy=scalar-grammar-v1` to the backend
fingerprint so the added eligibility/grammar policy stays auditable.
Historical projections are not rewritten by this change. Unsupported language
and unknown cardinality remain sourced facts. Arbitrary Anno type errors remain
possible; the measured scalar filter and grammar fallback are not a complete
NER classifier. No GLiNER or additional hardware is required.

## Anno spot check

Three synthetic examples were inspected before filtering: a Russian workout
line with `95%` and an ISO date; an English sentence naming Alice Smith, Bob
Jones and Acme Corp plus an ISO date; and a Russian sentence naming Иван
Петров plus `95%`. The independent expected entities were the four named
people/organization mentions. Before filtering, Anno returned those four and
four scalar mentions; after the scalar filter, the expected four remain and
the four scalar mentions are suppressed (mention precision 4/8 to 4/4, recall
4/4 in this tiny set). This is not a representative precision estimate and does
not justify classifying arbitrary workout words. The checked-in nine-case
extraction suite reports unchanged aggregate entity precision 0.6, recall 1.0,
F1 0.75 for baseline and linked-name revision.

## Execution decisions and remaining boundaries

1. The original checkout's unrelated Cargo.lock edit was preserved; the isolated
   checkout uses the HEAD lockfile. A dependency-sensitive baseline may therefore
   differ from that unrelated local edit.
2. Controlled whole-operation profiling preceded detailed claim-stage attribution.
   The reproduced false-warning amplification justified the local change; other
   production latency contributors remain unknown.
3. An initial colon-only eligibility rule was provisional and was replaced by
   schema-family eligibility and explicit complete records. Unsupported generic
   attributes retain their facts but lose comparable projections.
4. Automatic scalar rejection is limited to demonstrated ISO dates and percentages.
   Arbitrary Anno type mistakes remain possible; manual resolution is unchanged.
5. Initial leading-name binding and extra name reads failed acceptance. They were
   replaced by complete subject matching, neutral grammatical candidates and reuse
   of the existing pre-persist name map. Unsupported grammar remains unprojected.
6. Successful warning-body reads are cached per extract; failed reads are retried.
   Unique counterpart reads and complete response size remain proportional to the
   number of genuine relations. No candidate pagination or public cap was added.
7. The nested-reporter regression asserts durable subject identity, because warning
   absence alone missed the defect. Exact grammar deliberately favors skipping
   unsupported claims over assigning a reporter or compound member's identity.
8. Missing grammatical subjects use the existing resolver with neutral concept
   type; no person type is inferred. Quantity scale arithmetic is checked and exact.
   Identity still follows the existing resolver contract.
9. Pre-persist lookup failure aborts before fact creation; retry is verified. There
   is no new post-commit query or new retry-job policy.
10. Policy fingerprints are versioned. Historical derived state is not retired by
    repeat extraction or by closing a valid source fact; recovery remains separate.
11. Interdependent policy, fixtures and evidence are committed atomically rather
    than at the provisional five commit boundaries. This produces a larger review
    unit, while keeping the committed revision green.
12. The implementation stays in its local branch/worktree. Production behavior
    remains unchanged until rollout; no integration or historical mutation is
    inferred from implementation authorization.

No minor review findings were deferred. Measurement gaps and Anno limitations
above are explicit limits of the evidence and the supported contract.

## Final local verification

Final source verification on 2026-11-03:

- `cargo test -p memory_mcp --locked --no-fail-fast`: 2,601 passed,
  zero failed, 42 ignored across 153 test targets including doctests. Ignored
  performance profiles were run explicitly as documented above; other ignored
  tests retain their existing environment requirements.
- `cargo clippy --workspace --all-targets --features
  fs-watch,mcp-apps,streamable-http --locked -- -D warnings`: exit 0.
- `cargo fmt --all --check` and `git diff --cached --check`: exit 0.
- Unchanged extraction/reconciliation evaluation: 50/50, entity P/R/F1
  0.6/1.0/0.75, claim P/R/F1 1.0/1.0/1.0.

Full-suite and lint logs: `/private/tmp/extract-quality-verified-full-v3.log`
and `/private/tmp/extract-quality-verified-clippy-v3.log`. No dependency,
lockfile, migration or public MCP-tool surface change is included.
