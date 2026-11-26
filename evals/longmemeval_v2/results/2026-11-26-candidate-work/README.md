# Actual candidate work — 26 November 2026

Development diagnosis on exposed full-history failures, native Apple Silicon.
The `topical-tabs-*` series is final: original primary pair2/2, native gate8/8,
all seven reader cases scored4/7 (previous complete capture3/7). Earlier entries
below are a chronological ledger, not current acceptance.
No-model retrieval captures preserve whole histories and original acceptance;
separate `original-seven-post-*` captures use standalone local MLX, never LM Studio
or remote inference. The completed pre-tab replay scored3/7, not a retrieval score.
C2 remains NO-GO; source presence, answer accuracy and citations are separate.

## Frozen inputs and acceptance

Each variant's `registration.json` pins exact query, original whole-store hashes,
source identity, complete reviewed serialized span, source hashes and30s read /45s
process limit. A fresh complete disposable copy is used per process. Copy time is
outside the process/read clocks. `capture-identity.json` pins the optimized test
binary; `build.jsonl`/`.log` retain the build command. Six original snapshot files
are checked before and after each process (`snapshot-integrity.json`).

The existing `full_history_evidence` public integration target requires the
complete exact reviewed span in the final returned item from the registered source.
Existence at formation, rank, or in a partial receipt cannot pass it. SIGKILL and
read timeout remain resource failures; missing selection/packing is unknown.
CPU samples are bounded observations of the owned child, not whole-read stage
percentages. Test assertions, not a retrospective change to gold, decide success.

## What was actually fetched

`before/` adds telemetry only to afcce64c. Metadata is flushed immediately at the
actual fetch boundary, before policy/type admission. Counts include duplicate
occurrences across term groups. Content/key bytes are UTF8 field bytes, excluding
quotes, JSON syntax and engine/allocator overhead. They are not wire bytes or peak
RAM. Unique contents and source IDs use exact borrowed equality, not hash digests.

| Case / boundary | Rows | Content bytes | Key occurrences | Key bytes | Unique contents / sources |
|---|---:|---:|---:|---:|---:|
| dropdown fetched |874|351551353|5954171|120856614|96 /96|
| dropdown first-ID union |242|119580400|2148886|47006223|96 /96|
| toolbar fetched |2859|1072017514|18202475|368436183|126 /126|
| toolbar first-ID union |343|156007247|2933673|63673939|126 /126|

Largest fetched content1048542 bytes; largest key266 bytes. These are candidate
pools, not the number of admitted, selected or correct evidence items.

`FULL_HISTORY_WORK` counts actual completed dictionary constructions. In `after/`,
dropdown242 rows construct96 content dictionaries and96 key-list dictionaries;
toolbar343 construct126/126. Distinct fact IDs with equal raw content/key lists
share features. This is **once per exact key list per lexical pool**, not once for
an entire multi-query read: the toolbar alias pool is independently recorded as
343/126/126. Original and expanded queries are not skipped as equivalent. Fusion
and selection have separate immutable caches. No cross-read cache is introduced.

`FULL_HISTORY_PHASE` records cumulative read-relative completion times, not
isolated stage costs. `candidate-summary.json` contains only small metadata from
the retained raw logs. The source-filtered raw receipt can contain full text and is
retained only in explicit diagnostic artifacts, never production logging.

## TDD and interpretation

Raw controls are retained under `verification/`. Early fixture/setup failures and
passing characterizations are distinct from runtime REDs. Thread-local allocation
counts are calls, not bytes, timing or whole-process totals; application controls
pause the counter around every storage-port operation. Owner-query controls include
storage work on that caller thread. Never compare those two measurement scopes.

- Index vocabulary:50031 RED ->18052 intermediate GREEN, fixed30000 bound; latest
  whole control4032. Literal2004-term vocabulary and128 fixed-seed set/original
  vocabulary comparisons preserve membership and per-key preprocessing boundaries.
- Key-heavy public read: application RED2985917 ->793328 intermediate GREEN,
  fixed1000000 bound. Latest139160. Exact five IDs and source quotes preserved.
- Punctuation long source:2524616 RED ->644340 intermediate GREEN; latest71368.
- Unbroken long source:1963475 RED ->469645 intermediate GREEN; latest332214.
- Repeated-query200KB sources:1359843 RED ->1159587 ->1034597 RED ->699357 GREEN;
  latest693072. Same1000000 bound and exact five IDs/2048-byte source excerpts.
- Growing FTS union:64-record/shared-hit control passes142936 (not RED).
  Independent256-record/eight-term control237351 RED ->118934 GREEN under200000.
  This is a resource control, not proof of full-history latency improvement.
- New batch wide-sort regression: actual EXPLAIN from the public owner read
  observed `Filter` instead of `SelectProject` before sort. Nested projection
  restores exactly `record,fact_id,t_valid,ft_score,term_rank` before each term's
  original limit. The first aggregate variant is retained even though it failed.
- A literal wide-SQL oracle checks every returned field and first score across
  nine terms/two batches, starting beta before alpha to oppose identity order.
  Each term is limited independently; no refill after deduplication. First-hit
  aggregate and hydration execute in one statement's read view per batch.
- Public different-winning-span control requires grounding0.75. Reusing the
  original query's winning sentence for the deduplicated query yields0.55 and is
  detected by a deliberate mutation. Both original and unique scores share one
  normalization pass but independently maximize their own sentence scores.

Intermediates are exposed development results. They cannot establish independent
confirmation, a population accuracy estimate, or an overall memory benefit. The
original unsuccessful C1/C2 decisions remain unchanged.

Final full-history results and delivery verification are recorded below when
completed; a successful resource control does not close the full-history gate.

## Graph diagnosis and bounded reads

`narrow-batched-sort/` still fails both original process bounds. Its dropdown
capture completes channels at21020.523ms and fusion at25829.190ms; admission and
packing are unobserved. Toolbar performs a second alias lexical pool and has not
completed channels before death. These are cumulative read offsets, not stage
CPU measurements. Dropdown's78 edge reads sum to10.066s of observed storage-call
durations. Their real engine plans already use endpoint indexes.

An `IN`-predicate prototype was rejected: it lost endpoint-index selection in
actual EXPLAIN. Original graph SQL and real-engine index assertions were restored;
the rejected diff and failing controls are retained under `verification/`.

`bounded-graph/` overlaps at most four original32-seed read batches. Successful
results retain original batch order; the first observed error fails the read and
drops sibling futures. This explicitly changes error observation from sequential
batch priority to fail-fast completion. No task is detached, no frontier/hop limit
or candidate cap changes, and each query retains its original temporal predicates.
Public controlled tests observe RED1 versus4 simultaneously started batches,
then GREEN overlap/bounded waves. A second runtime RED exposes `join!` waiting on
siblings after failure; `try_join!` returns the original storage error promptly.
The cancellation control proves owned futures drop at the `DbClient` boundary;
it does not prove an already-dispatched engine query has stopped internally.
Six real-engine graph controls and401 retrieval controls pass. A separate
nonempty reverse-completion control requires the first sorted public graph path.
Deliberately appending the fourth batch first changes that path and fails the
literal oracle; restored order passes. Cancellation/error/overlap/order controls
are four independently named public-read tests.


## Admission reuse and temporal bitmap diagnosis

A32-fact public read with2000 shared index keys reproduced1142002 application
allocation calls under the existing1000000 bound. Reusing exact content/key-list
query matches in admission reduced this to383333, preserving all five selected
IDs and quotes. This is separate from the older8-fact control. The new
`admission.topics` work event counts completed actual dictionary builds; it does
not claim the lexical and admission stages share one global cache.

`bounded-graph/` remains0/2: process45.199/45.182s. Dropdown completes channels
at18800.382ms, fusion22671.738ms, admission27594.742ms; selection is unobserved.
Toolbar channels29279.201ms, topics29576.242ms, no completed fusion. Overlapping
edge call durations sum to31.833s/30.244s; these sums are not wall-clock costs.

Pinned SurrealDB4.0.1 diagnosis rejects endpoint `IN`, forced index hints and a
plain nested SELECT: observed plans lose endpoint indexes or still build the
global `edge_t_valid` bitmap. All rejected prototypes are archived. A temporary
isolated actual-snapshot query compares the first32 actual entity links, not a
filtered quality evaluation. Original129.569ms, materialized33.290ms,3222 identical
ordered rows. Fixed before/after execution order confounds warm cache effects;
this is feasibility evidence, not an unbiased speedup estimate. The disposable
clone and six original snapshot hashes are verified. See `graph-query-isolation/`.

The permanent real-engine plan regression first fails on the old global bitmap,
then passes with endpoint candidates materialized inside `array::filter` before
the complete temporal predicate. Both endpoint indexes and absence of a table
scan are checked on the actual inner statement; outer EXPLAIN's opaque
FunctionCall alone proves nothing about its internals. No edge cap is added.
Future-heavy neighborhoods can materialize more rows before filtering; the
unchanged full-history gate and temporal controls remain mandatory.

Final incremental review found no Important/Critical defect. It notes missing
explicit legacy-null-ingestion/order-tie fixtures; existing temporal predicates,
hostile IDs, invalidation boundaries and32-seed row-retention controls pass.
`indexed-graph-admission/` still fails strict acceptance0/2. Dropdown now completes
in27290.875ms: ranked source15270.380ms, episode candidates21661.150ms,
selected27072.900ms retain the block, packed27290.180ms loses it. Toolbar times
out30004.957ms. `query-sized-scoring/` also0/2: dropdown completes27907.696ms
but loses the block during packing; toolbar times out. These expose a separate
packing defect after the earlier resource failure, not a quality success.

## Repeated-topic work and borrowed preprocessing

The initial admission event missed hidden dictionary rebuilds in repeated-topic
checks. A separate public32-experience/2000-key test reproduces1271976 allocation
calls (bound1000000), then passes375530 after sharing the raw vocabulary and299555
after borrowed preprocessing. Its literal five IDs and quotes remain unchanged.
Observation32 now267335; these are separate scenarios, not interchangeable data.
`admission.topics` counts actual raw and URL-filtered builds: one without URLs,
two when filtering requires distinct semantics. Lexical, admission, fusion and
selection retain separate scopes; this does not prove one global build per read.

A public unchanged-input vocabulary control fails10022 calls under1000, then
passes7 after Cow preprocessing and borrowed raw-record fields. Necessary quote
matching, episode reference preprocessing, Russian stemming, per-key boundaries
and literal membership remain covered. Counts are caller-thread allocations,
not heap bytes, storage allocations or latency.

## Complete groups and independent negative controls

A separate public source with a narrative query and complete archive buttons
3100bytes later reproduces the packing defect. Explicit UI enumeration now ranks
complete fitting query-bearing groups, rather than requiring intersection with
the narrative window. Ordinary narrative requests and oversized groups retain
existing behavior; original source offsets, quotes and2048byte cap are preserved.
The first global-group variant has a review-discovered regression: generic
`More options / Close` can displace a Falcon deployment answer. A public test
fails before requiring support beyond generic UI trigger terms, then passes.
All10 positive/negative packing scenarios pass. This rule uses no gold labels,
source IDs, reviewed offsets or distance tuned to an answer.

`borrowed-dictionaries-complete-groups/` is registered before its one capture per
case. Every original criterion remains in force; outcomes are recorded below.


`borrowed-dictionaries-complete-groups/` strict0/2: dropdown completes26502.918ms
but chooses a distant sorting menu at packing; toolbar30003.803ms timeout after
admission28088.557ms. Actual admission dictionary counts are228/302 (two vocabularies
per114/151 exact lists with URLs), correcting the earlier hidden-work omission.

A valid independent multi-state source reproduces the stronger-sort-menu error.
The first fixture accidentally indented its opening control by1300spaces; that
setup error is not counted as a product RED. With a separate newline and valid
control depths, lexical-first ranking fails; topical eligibility plus proximity
before lexical tie-breaks passes all11 packing scenarios. No maximum distance
is tuned to a reviewed span. The original lexical winner anchors UI-state coherence.

The public index-only-anchor control retains32 facts/2000keys, the five literal
selected identities/quotes and the original1000000 application-allocation bound.
3054835 RED ->2974113 still RED after sharing anchor/selection memberships ->452628
GREEN after also caching exact content/key-list reference extraction. Reference
admission and returned evidence reuse those immutable references. URL/reference
semantics are checked by the unchanged401 retrieval controls. All14 resource
scenarios pass. These are application allocation calls, not end-to-end timing.

`coherent-groups-selection-memberships/` registers one new full-store capture per
case after those diagnosed repairs. All earlier attempts remain failures.


`coherent-groups-selection-memberships/`: dropdown strict PASS25844.959ms, full
reviewed block survives every recorded stage; toolbar still times out30003.677ms.
Its admission completes27407.573ms; the subsequent episode substring batches
complete around28419/29290ms before timeout. This is observed work, not an inferred
index-key bottleneck. Cumulative phase times are not isolated CPU measurements.
There are44 total exposed development processes including20 preceding this packet.

Episode fallback has the same repeated-wide-payload pattern as fact FTS. Public
owner RED: eight returned payload occurrences for two first-hit winners. The
replacement `select_episode_candidates` returns a flat first-hit union and removes
the sole-consumer grouped API. Each term keeps original substring/lowercase,
bi-temporal predicate, independent cap and order;8-term RPC bound remains.
Prior IDs are excluded after caps, so no refill is possible. Complete fields are
hydrated once in the same statement. A literal wide-SQL oracle and a ninth-term
new-winner/initial-ID/no-refill/transaction-time control pass;401 retrieval pass.
`unique-episode-hydration/` registers one bounded new capture each after this repair.


`unique-episode-hydration/`:0/2, both external45s kills. Dropdown reached
selection24440.792ms, then synchronous access writes43.009/2122.652/1760.025ms;
this observes write cost, not a proven index-rebuild cause. `RETURN NONE` public
RED230475 returned bytes -> GREEN2 while durable count/time/content, concurrent
increments and vector isolation pass8 controls. Episode actual EXPLAIN mutation
Filter RED -> restored SelectProject GREEN protects thin-sort input.
`heat-write-no-afterimage/`: dropdown strict PASS26999.960ms, toolbar30004.378ms
timeout. Writes remain synchronous; no durability/error criterion was removed.

Ordered scoring reuses normalized token storage for exact prepared raw tokens,
retaining every occurrence/order and original sentence boundaries. Public8-source
long-word resource RED2494009 -> GREEN23226 calls under unchanged1000000 cap;
mostly-unique opposite input553066 calls, same bound, exact IDs/text retained.
No claim that unique-word latency or all inputs improve.401 retrieval controls,
including128 fixed-seed independent scalar scoring cases, and16 resources pass.
`shared-ordered-tokens/`: dropdown strict PASS25903.894ms; toolbar30009.059ms
 timeout after admission28503.739ms. A separately authorized native1s CPU sample
at process~40s shows admission/index token normalization; sample wall interval and
footprint describe this point only. Sandbox sample attempts failed permissions;
only `toolbar-native-cpu-sample.txt` is the successful sample. No unrelated host
process was modified.50 total development processes including20 earlier attempts.


`IndexTokenVocabulary` now shares exact raw-token normalization across distinct
key lists, with a per-operation 8192-entry /1MiB UTF8 payload memo bound. Overflow
still contributes every normalized term; a20000-distinct-term control verifies
this. Public64-fact/2000-common-key resource RED1219530 -> GREEN838984 application
allocations under the unchanged1000000 bound. The memo limit is not a source,
candidate, vocabulary or whole-memory limit.
`bounded-index-token-reuse/`: strict0/2. Dropdown external45.166s kill after
selection24226.263ms; toolbar read30142.317ms timeout after admission30124.117ms.
All32 processes in this packet plus20 preceding processes remain exposed
**development** attempts (52 total). None is a confirming study.

Toolbar telemetry shows two whole lexical reads for app-equivalent action/actions.
Base-only alias elision requires identical ordered application tokens, identical
nonempty native token streams, the exact known stage-independent analyzer chain
and both actual fact-index bindings. Real edgengram and different content-index
analyzer cases each fail before their respective guard (one read instead of two)
and pass after it. Unknown/failed/incomplete/empty analysis retains independent
queries. A different key-index analyzer and genuine/native-only aliases are
opposite controls. All10 alias and17 resource controls pass. Only aliases
identical to the base semantics are elided; independent aliases are not merged,
so their existing ranking contributions remain. This is a measured duplicate-work
repair, not yet a full-history acceptance result.


Final alias hardening adds a real additional FULLTEXT-index regression:
1vs2 reads RED -> exactly-two-known-FTS guard GREEN. All11 alias controls pass.
`equivalent-base-alias/`: dropdown strict PASS28385.937ms (process44.660s), toolbar
external45.113s kill. Toolbar has one343/126 lexical pool, channels14323.330ms,
fusion18838.642ms, admission21472.667ms; eleven episode batches then take
731/736/732/738/743/759/747/874/861/823/575ms. No selection/final receipt means
packing and answer remain unknown, not passed. All original snapshot/source hashes
checked unchanged after both processes;54 exposed development processes total.

A proposed episode-owner allocation-traffic control (four65KB sources/eight
substring terms, same four literal IDs/full contents) measures33828850 requested
bytes, excluding setup. Two initial materialized membership variants are worse:
44843167 and47771962bytes; neither is accepted or used in a full-history capture.
These are caller-thread engine allocation traffic, not peak resident memory or
whole-process cost. The proposed fixed32-times-returned-raw-payload resource
bound remains unchanged during this investigation.


Episode resource-method correction: the initially proposed absolute32x raw-source
byte ratio was not an accepted duplication criterion: a one-term read itself
costs roughly5.5–9.4MB in engine allocation traffic, depending on prior planning.
Retain all exploratory failures, including dynamic-matrix intermediates. The
permanent contract compares eight terms then one on the same four65KB sources,
with exact full-row equality, bounding added seven-term work by the same32x raw
payload allowance. Corrected baseline runtime RED:33828850 vs5521057bytes,
additional28307793. Matrix GREEN:10264645 vs8680754, additional1583891.
Fixed-order caller-thread requested bytes exclude other engine threads and do
not establish unbiased timing, RSS, or full-history acceptance. The original
30s/45s and source-span criteria remain unchanged. The matrix computes exact
substring membership once per visible source per eight-term RPC, retains thin
identity/time/boolean rows for independent term sorting/caps, then hydrates
first-hit IDs in the same statement. No source bodies enter per-term sort.


`episode-membership-matrix/` completes both reads: dropdown25644.614ms,
toolbar29607.649ms, processes41.491/43.968s. Strict result remains1/2:
toolbar reviewed source survives ranked/episode candidates but is absent in
selected_pre_packaging and final context; this is observed selection loss,
not packaging loss or timeout. All source/snapshot integrity checks pass.
56 exposed processes total. No answer run/C2 admission. Review's partial-hit
resource gap now covered: one hit/seven absent, same four65KB sources, runtime
17058415 vs8678753 bytes, additional8379662, passes original32x added-work
bound. This is narrow headroom; partial closure capture copying remains real,
so do not claim all term-wise payload copies eliminated. No source change made
during capture. Next independently reproduce explicit-anchor selection against
generic repeated histories before a product change or another full capture.


Review gap resolved through a stronger mixed-hit control: seven matches/one
absent reproduces runtime RED8645874 added bytes. Replace only the partial-path
capturing closure with at most eight generated borrowed predicates. All-hit,
one-hit and seven-hit controls GREEN (added4.16–4.48MB under unchanged bound).
Do not claim the prior all-hit figure1.58MB describes this final source.

Independent selection regression: short generic query passes before fix (not a
RED); faithful contextual question plus response-format suffix fails with seven
generic archive distractors, three independently named controls and budget5.
Root cause: lexical_search_surface recognizes question starters only at the
first word; `On ... , how many ...?` falls through to the entire request,
including answer-format instructions. Recognize interrogative/request starters
after comma/colon only in a question-terminated clause, preserving that entire
question clause and the unchanged raw query for temporal/admission. Runtime
RED -> GREEN without changing ranking weights, IDs, term/source caps or gold
spans. Existing multi-question/initialism/negative-directive controls must pass.


`contextual-question-surface/`: both processes exceed unchanged45s child gate
(45.194/45.157s), strict0/2; selection completes24273.887/27862.904ms but
final context remains unknown. Dropdown completed two sequential heat writes
at24573.493ms (53.098ms) and26731.728ms (2158.205ms); a later write never
completes before kill. Toolbar stops after claim attachment before a completed
heat write. Snapshot/source/binary checks all true.58 exposed processes total.
Native sample could not attach; do not infer a stack from an absent sample.
Pinned Surreal4.0.1 index_step compares old/new values and skips unchanged
index maintenance, so reindexing is not established as the cause. Isolate
whole-row expression/schema work in the public access-store seam before a fix.


Bounded access wave: public barrier runtimeRED1 started versus4, GREEN4
concurrent writes, next wave awaited, all pending writes drop with read, middle
failure does not cancel siblings/later writes, real-store5/5 durable counters
before return. Duplicate-ID/failure ordering existing control passes. Isolated
whole-store access probe retained (not full-read qualification):25k-key writes
1.81/1.895s; scalar arithmetic alternative did not help and was rejected.
`bounded-access-waves/` remains0/2 strict,45.146/45.181s external kills; selection
24.012/27.371s. All snapshot/source/binary checks pass.60 exposed processes.
Actual native CPU sample at40s is now available: substantial episode scoring
normalizes composite serialized tokens. No improvement is inferred from absent
final receipts; the access-wave behavioral guarantee alone passes.

New independent resource shape keeps changing numeric IDs inside repeated word
parts, through public context assembly. Exploratory10k-ID case18,324,687
allocations retained but not proposed as a million-call size contract. Permanent
fixture uses2000 distinct IDs, matching existing mostly-unique public budget:
runtimeRED3,669,820 allocations exceeds unchanged1,000,000. Investigate
context-free word-part normalization; preserve all order/count/reference
semantics, no answer/domain/source predicates.


Composite normalization follow-up: broad shared vocabulary/ordered memo proposal
was not sufficient and its extra vocabulary wrapper was removed. Measured
allocation stacks identify ordinary word-part normalization and reference
intermediates across lexical/admission/selection/packaging. Keep exact set
vocabulary while normalizing each distinct ordinary word once; keep every
ordered occurrence, with bounded operation-local text token reuse across one
lexical pool, episode rank or excerpt search. No score, limits or source rules
changed. A full21-case resource run exposed a measurement defect introduced
by concurrent storage calls: an allocation-pause guard held across await can
restore thread-local counters out of order. Four invalid measurements are
retained; pre-correction figures on this concurrent path are provisional, not
an accepted quantitative improvement. Pause storage allocations only during
each individual future poll; all20 existing controls pass with this meter.
New composite fixture is a valid runtime RED1,004,216 under unchanged1,000,000;
remove its remaining per-token temporary reference vector, then rerun all21.
Observed another repository compiling on host during development; do not
attribute earlier timing failures to it without contemporaneous evidence.


Corrected resource meter counts only public caller polls, pausing each storage
poll, with no runtime tasks counted while caller suspended. Matched original
normalization runtimeRED3,669,865; candidate838,002 under unchanged1m cap.
22resource controls pass after review's Kelvin-sign Unicode RED→GREEN.
401retrieval/87shared/67other public controls pass. Allocation traffic is not
RSS or latency. `composite-token-reuse`1/2: dropdown25331.736ms complete block,
process41.503s; toolbar completed30044.745ms but target lost selected/final,
process101 plus wait-budget flag45.127s (not proof of kill).62 exposed processes.
Snapshot/source/binary checks true. Full-history diagnostic now explicitly
rejects an elapsed-over-budget Ready, beyond cooperative timeout semantics.
Question-fusion regression first final-only proposal passed (episode rescue,
not fusion proof); first stage fixture invalid because add_fact ingestion now
was after historical as_of. Fixed both axes explicitly; genuine runtimeRED
grounding0.75 vs0.234615 for same question plus formatting. Fusion now uses
existing question surface for features/grounding/matches, raw text retained for
temporal/admission/first-person/rationale.13source/evidence controls GREEN.
New `question-surface-fusion` capture frozen but not started until existing
verification child finishes; own verification coordinator paused93069 to avoid
launching heavy checks during timed capture and MUST be resumed afterward.


Continuation audit: `question-surface-fusion/` completed one registered pair,
strict1/2. Dropdown read22750.975ms/process38.446s PASS; toolbar read27477.242ms/
process42.084s FAIL (source in fusion/episode pools, absent final). Both reads
completed; no external timeouts. This localizes loss between fusion and final,
not yet uniquely to MMR: admission was not separately observed.64 exposed whole-
store processes now retained. First launch failed at build-log JSON parsing before
any read; overlapping/trailing build output is archived separately, not counted.
A graph-navigation request made during the second process did not finish and was
cancelled; these are no controlled latency-effect measurements.

Review corrected elapsed oracle: read clock starts immediately before timeout,
after count_episodes/START/capture setup; synchronous Ready after30s still fails.
Verification coordinator93069 resumed and finished: production2853/0failed/45ignored
(pre-fusion source), harness929/0failed/22ignored, both clippy/build/fmt/diff exit0.
These are dated checks, not final-current-source coverage after later diagnostics.

Important fixture erratum: the earlier contextual-question competing-source test
used add_fact(now) with as_of2026-05, so its facts were invisible. Prior RED/GREEN
still tested episode behavior, not competition against visible fact candidates.
Fixed durable t_valid/t_ingested; missing entity_links/provenance schema proposals
are invalid setup failures, retained separately. Correct fixture runtime RED loses
the reviewed source. A bounded quoted-support boost was tested, failed, and fully
removed; it is not part of the candidate. A new public diagnostic contract first
failed for absent admitted_fact_candidates, then passed;13/14 public scenarios
pass with the competing-source regression intentionally still RED. Its focused
source is absent already at ranked_fact_candidates, so it does not establish the
same exact causal stage as the full toolbar case. Do not conflate them.
`admission-boundary/` freezes the same full-store pair with a real post-admission
observation and selective source-only diagnostic conversion. No ranking policy or
source criteria were weakened. Await this causal evidence before another policy fix.


## Continuation: precise admission boundary

`admission-boundary/` completed1/2 (dropdown22843.532ms; toolbar29152.978ms,
process exit101 and outer wait-budget flag). Its historical admitted stage was
AFTER subject filtering, admission AND candidate cap; it cannot uniquely identify
admission rejection. The new `precise-admission/` separates those boundaries.
This one scheduled pair is1/2 strict: dropdown20835.009ms/process35.625s PASS;
toolbar24782.062ms/process38.065s FAIL, no outer timeout. Toolbar source survives
formation, fusion, subject filtering and actual admission, then disappears after
candidate bounding. Full episode source is observed, but final context lacks it.
Source/binary/snapshot integrity true.68 exposed whole-store processes completed.
These are development diagnostics, not a controlled latency effect or confirmation.

Public negative control found that episode rescue admitted a source after the
fact path rejected substantive post-question requirements. It now uses the raw
query with conservative removal of recognized presentation-only clauses. Unknown
words/constraints remain. The broad question-only admission proposal was removed.
14/15 source controls passed; the remaining synthetic case has an additional
initial/fallback winner-pool defect. Actual inventories: initial7sources excludes
target; independently bounded fallback8sources includes it. Union now preserves
per-row native FTS identity; fallback cannot become native by union membership.
The complete final-answer criterion for that synthetic fixture is still open.

Review found the first conditional-When guard also swallowed a genuine compound
date question. Public `compound_when_question_requires_event_time_from_the_source`
was runtime RED then GREEN after preserving interrogative auxiliary first clauses.
Existing conditional dropdown regression is the opposite control. Neither grammar
rule is a claim of complete language understanding.

Next registered hypothesis `diverse-candidate-bound/`: use existing query-anchor
and source-diverse MMR at the same intermediate cap instead of taking a relevance
prefix. Original query, complete block oracle and30/45s limits unchanged. Do not
accept this proposal unless current tests and full-history evidence support it.


`diverse-candidate-bound/` completed1/2: dropdown28534.990ms, toolbar31601.504ms
(timeout/late partial),70 exposed processes total. Review found that the final
MMR selector also prunes and applies final grounding floors before later temporal
and reranker stages; this is unsafe as an intermediate cap. The proposal is fully
removed, regardless of observed results. It also failed the original30s criterion.
Next independent test targets named source phrases versus scattered words and
index-only labels; no weight is tuned against the closed answer.


## Control-label identity: reviewed development candidate

The early-MMR intermediate cap experiment is rejected and fully removed. It
applied final pruning/floors before later temporal/reranker stages and failed the
original deadline. Its two processes remain archived;70 whole-store processes
precede `control-label-identity/`.

A public lexical-channel fixture observes7 initial sources (target absent) and8
term-fallback sources (target present). The winner-only policy discarded the
latter. The union retains both independently bounded channels, first-ID order
and the primary row's numeric score. Only actual initial FTS IDs receive native
provenance; native identity does not imply the retained numeric score came from
the initial query. This test is now explicitly named
`losing_lexical_pool_retains_source_without_native_fts_provenance`. Its earlier
final-answer assertion was overbroad: its short source lacks question framing
and its bare English-number suffix is not a recognized presentation clause.
The old runtime failures remain archived; union alone does not prove final recall.

A separate eight-source, fixed-valid/ingestion-time public test reproduces final
loss (wrong source0). It requires the target to reach actual admission first.
Observed complete UI labels now outrank scattered topical words for explicit
positive quoted control-membership questions, consistently in fact selection and
episode rescue. Index keys alone and longer alphanumeric labels do not count;
the distractor has greater confidence. No hard admission, temporal/access,
source/candidate cap, deadline or independent reranker override is added.
A configured reranker disables this preference in both channels. MMR compares
coverage classes with a total order and retains the true maximum relevance.

Replacement/context quotes and negated membership fall back to ordinary ranking.
Public runtime REDs for not/no longer/except/excludes are retained, followed by
23/23 public stage controls GREEN. This finite conservative grammar is not a
complete natural-language parser. Review closed the comparator, reranker,
MMR-normalization and negative-scope findings before the full capture.

An exploratory32-source fixture failed before actual admission because the
reviewed source was not fetched. That is a real candidate-fetch coverage limit,
not fixed by this selection rule. Its log is retained and is not reclassified as
a final-selection success. The canonical two whole-history cases retain their
original full-span criteria, exact queries and30/45s bounds.


`control-label-identity/` actual result: dropdown21938.850ms/process36.541s PASS;
toolbar27032.403ms/process40.721s FAIL, no external timeout. Integrity true.
72 whole-store processes retained. Toolbar reviewed span now survives actual
admission, bounded candidates and selected_pre_packaging, then is missing in
packed_context. This fixes the measured source-selection boundary but does not
close strict acceptance. Current original toolbar source includes an all-visible
nine-control group and a later hidden copy plus visible filters; existing nearest
lexical-window packing chooses the latter. Next public TDD isolates visibility
of requested labels within complete bounded groups; original gold stays unchanged.


## Visible-control packing candidate

Source-linked `control-label-identity` evidence locates the remaining toolbar
loss after final selection, at excerpt packaging. A public independent two-copy
fixture reproduces the same failure: a distant visible named group loses to a
near hidden copy plus unrelated visible filters. Runtime RED then GREEN.
Packing now ranks requested labels supported inside single explicitly visible
button names ahead of existing distance/lexical tie-breaks, only for the existing
explicit positive quoted-control question class. It keeps complete-group
eligibility,2048-byte cap and source-contiguous byte offsets. Unknown/tied
visibility and explicit hidden/invisible questions keep the previous ordering.

Review found and public tests reproduced two false-support bugs in intermediate
proposals: joining separate names into one label, and treating visible inside
quoted attribute/escaped label text as a top-level attribute. Both are repaired;
31 public stage controls and22 unchanged resource controls pass. A quote-aware
scanner handles escaped single quotes and quoted attribute values; malformed or
unclosed values give no positive visibility evidence. These are finite supported
accessibility-role conventions, not general UI/NL understanding. Review has no
remaining Critical/Important findings in this scoped change.

The failed compilation in visible-controls-reviewed-green.log did not reflect
missing current APIs: source inspection found them present. A forced current
library rebuild by updating lib.rs mtime resolved stale compiled metadata; no
source API or test was removed. The cause of that stale metadata is not proven.
Concurrent local Cargo builds caused lock waits and are not stopped or treated
as quality results. No controlled latency effect is claimed.

`visible-control-packing/` freezes one new original pair after these behavioral
changes, source/tests hashes and exact previous oracle. Its result is pending.


## Packing diagnosis correction

`visible-control-packing/` completes1/2 strict: dropdown22291.100ms/process38.879s
PASS, toolbar29043.200ms/process43.840s FAIL; neither outer timeout.74 processes.
Source/binary/snapshot integrity true. Toolbar full span remains selected, but
packing picks a different state. Earlier description of hidden buttons was an
incorrect human interpretation of the raw row. Both states explicitly mark the
nine named buttons visible; state38 has explicitly visible main and closes with
Personal Information, state39 main visibility is unknown and adjacent filters
are merged by flat grouping. No hidden/recency assertion follows from unknown.
The child-visibility fix has valid independent public regressions but alone does
not close the actual whole-history failure.

Next bounded policy hypothesis uses nearest semantic-container confirmation
only as additional positive observation evidence after visible requested labels.
No control count/density or expected answer enters ranking. A raw state/time
qualifier disables this parent preference: stronger annotations do not establish
recency. Public requested-later and unknown-inner-container opposites pass before
the implementation. State-specific retrieval/formation remains a separate debt;
the conservative finite guard is not universal temporal-language understanding.

### Confirmed-container packing — bounded development capture

Public stage36/resource22 pass. A nearest unknown semantic container (including
bare `group`) blocks inherited visibility. Explicit temporal selectors, recognized
dates/relative dates and supported state words disable the parent tie-break;
unknown remains unknown. Quote-aware single-button-name evidence retained.
Independent static review: no Critical/Important findings, finite qualifier guard
limitation acknowledged.

`confirmed-container-packing`: exact original whole-store pair **2/2 strict PASS**.
Dropdown20,830.696ms/process35.548s; toolbar27,873.012ms/process41.319s, both exit0,
no outer timeout, full registered source span in one final item. Source/binary and
six-file snapshot integrity true; unchanged30s/45s and no reruns. This series now
retains76 bounded processes (56 candidate-work plus20 earlier narrow/stage).
No causal latency/population claim. Step4 closed; native8, final full regression
checks and original-seven reader/judge are still pending; C2 remains NO-GO.


## Проверка диалоговых регрессий перед окончательной приёмкой

Полный production-прогон после `confirmed-container-packing` выявил11 реальных
регрессий `conversation_neighborhood` и7 сбоев FSEvents в sandbox. Последние
отдельно проверены нативно:18/18 сценариев файлового наблюдения прошли. Это не
исправление кода и не основание игнорировать диалоговые регрессии.

Чистый HEAD, пересобранный с явным `Compiling` из архивной копии исходников,
прошёл35/35 диалоговых сценариев. Две промежуточные проверки с общим Cargo target
оказались недостоверны из-за повторного использования бинарника другой копии;
они сохранены как invalid, а не используются для сравнения. Scoped Cargo clean
удалил артефакты только production-пакета, после чего свежий candidate снова
воспроизвёл дефект.

Причина: raw answer admission в раннем episode fallback отбрасывал реплики,
не утверждающие ответ самостоятельно, хотя они нужны для восстановления ответа
по producer-declared порядку диалога. Возврат только очищенного поискового запроса
восстанавливал диалог, но терял дополнительные смысловые условия; широкий список
новых anchors также отклонён после пяти провалов. Нормализация всех standalone
источников отдельно отклонена после провала compound-When контроля даты.

Текущий минимальный вариант сохраняет normalized candidate policy только для
источников с явным `source_context`; все смысловые clauses участвуют в словаре
кандидатов. Standalone источники сохраняют raw admission. Новые anchors, API,
запросы к хранилищу и лимиты не добавлены. Доступ, cutoff, явные временные окна и
raw structured-subject guard сохраняются. Это не утверждение полной эквивалентности
raw admission для диалоговых реплик; атрибуция ответа остаётся за downstream
source-neighborhood/final policy.

Итоговые36/36 диалоговых,36/36 стадийных и22/22 ресурсных сценария проходят.
Нативный полный production-прогон:2855pass/0fail/45ignored,176target results
(включая существующие architecture lints и doctests); обычная сборка отдельно
подтверждает36/36 conversation. Новый независимый двухрепличный
контроль дополнительного условия о канадской регистрации команды воспроизвёл
RED при мутации, отбрасывающей substantive tail, и GREEN с исправлением.
Workspace/diagnostic clippy, fmt/diff и расширенные source targets проходят;
harness выполняется. Предыдущие2/2 full-history относятся к исходникам до этой
поправки и требуют отдельной новой регистрации. C2 по-прежнему NO-GO.


### Смена исходников при финальной проверке

Регистрация `semantic-neighborhood` остановлена identity guard **до Cargo build и
до любого чтения**: четыре файла embedding-контракта/eval adapter изменились при
параллельной работе в master. Это invalid preflight с0 cases, а не провал retrieval
и не новая попытка на раскрытом эталоне. Список хешей сохранён в
`semantic-neighborhood/preflight-invalid.json`.

Завершённые production2855/0/45 и harness929/0/22 относятся к предыдущему дереву.
Они сохранены, но не объявлены подтверждением обновлённого дерева. Новый
`source-refresh-14d14b46` фиксирует актуальные хеши; весь финальный gate повторяется
с проверкой неизменности исходников после каждой команды. Правила retrieval,
исходный снимок, эталоны и30/45с границы не менялись. Число исполненных полных
историй пока остаётся76.

## Current source-refresh acceptance (26 November)

`source-refresh-14d14b46` pins the current 42 source/test files and passes both
original whole-store cases without changed queries, source spans or 30/45 s
limits: dropdown read21.942653 s/process37.676161 s; toolbar
read28.293557 s/process42.782779 s. Every completed source-linked stage retains
the full reviewed span, including one final returned item. Before/after snapshot
hashes and candidate source/binary identity match. The complete bounded series
contains78 processes (58 here,20 earlier); invalid preflights contain zero reads.

Final lexical pools:242/252 first-ID facts,96/97 exact contents/key lists and
96/97 actual dictionary constructions, once per exact list per pool. Admission
separately constructs228/300 raw+URL-filtered dictionaries for114/150 exact lists.
`read-cost-breakdown.json` records observed timing: toolbar's five durable access
updates occupy about6.23 s in two overlapping waves after internal selection;
actual final packing about0.328 s. Dropdown access interval1.89 s, packing0.145 s.
Do not sum overlapping SQL durations or infer an engine-level cause.

Fresh `verification/source-refresh-current/summary.json`: production2856/0/45,
harness929/0/22, scoped source/resource103/0/0 (pass/fail/ignored). Required
workspace/diagnostic clippy, fmt/diff exit0. Totals include existing lints/doctests.
Fresh release build pinned `ffa25f31044fc24be838f09972d3c33f95bb6af96c682097a9d4f0a7c87ee33d`.
`native-gate/comparison.json`:8/8, no_regressions,0invalid/unobserved; run55.946 s.

Original seven reader/judge evaluation is separately registered and running;
no answer acceptance is inferred yet. Its archive-source provenance preflight
initially rejected the parent project's git HEAD; no model or memory read had
occurred. All53 staged upstream files now match the retained commit archive;
this corrects provenance without changing scoring. Archived raw256-protocol
2/7 is an incomplete-score lower bound; archived concise512 strict2/7 is a
different prompt protocol. They must not be presented as a matched accuracy
baseline for the new original256 pass. See `analysis-registration.json`.

C2 remains NO-GO. The exploratory32-source fetch loss remains open; it is not
closed by the first-ID union or by passing these two whole-store examples.

## Tab-list repair follow-up

The first original-seven replay completed: native and independent answers3/7,
positive answers3/4, false-premise0/3; source presence v2 is4/7. Full original
6d reviewed-source span became absent, although the selected authentic4ae source
contains the same browse labels. Neither aggregate4/7 nor an alternative source
can turn that original gold-source miss into success.

Native diagnostics establish gold-source rank40 / first-source rank40 of148;
source-diversity deduplication is not a justified repair. Independent public
fixtures reproduced short link groups, nested lists, preceding buttons, mixed
button requests, overlapping groups and byte-cap behavior. The actual alternative
full-store preflight still failed: a prior sidebar list qualified only on the
structural role `list`, making a bounded union too wide. A public role-only
regression failed before the minimal topical-filter fix, then passed. Current
conversation36 / stage44 / resource22 all pass. Review found no important defect
within the finite pure-tab contract; inverse mixed role requests are not claimed.

`topical-tabs-preflight` now passes: one authentic complete block from its exact
registered source in one final item, process36.928s, unchanged30s/45s limits.
This is an additional exposed development control; original6d gold metric remains
unchanged. `verification/topical-tabs-current` and the release build are running.
Only after these finish, run the final primary pair, native8 and one preregistered
original-seven replay. Intermediate source-drift captures/gates are retained but
excluded from final-source acceptance; no unchanged reruns select success.

## Final delivery — topical-tabs series

Completed on the same42 pinned source/test hashes. Current acceptance roots are
`topical-tabs-final`, `native-gate-topical`, `original-seven-post-topical`, and
`verification/topical-tabs-current`. Earlier statements about running captures
above describe their recorded point in time.

- Original dropdown/toolbar2/2: read21.149/26.852s, process36.632/40.860s,
  unchanged30/45s limits, original exact source/span.
- Directory original gold still misses; authentic alternative exact source block
  is complete in native receipts and actual reader text. These are distinct claims.
- Native8/8, no_regressions,0invalid/0unobserved; final release SHA
  `8e3bc61dfc0cd4cf06de7a7a3014225e23e3d2b57c964a784ffec18e096340da`.
- Official scorer4/7 and independent closed answer4/7; positive exact sets4/4,
  false premise0/3. Original reviewed span formula v2 stays4/7→4/7.
  No terminal reader/judge errors; server attempt log records include retries
  and BrokenPipe lines, not exact per-case failed-attempt counts.
- Production2856/0/45, harness929/0/22, scoped111/0/0 (passed/failed/ignored).
  Required workspace/diagnostic clippy, fmt/diff exit0; Python55pass/5skip;
  five public protocol subprocess controls pass.
- Model servers stopped; temporary stores cleaned. Seven-case result is exposed,
  descriptive development evidence, no independent treatment benefit. C2 NO-GO.
- Exploratory32-source fetch loss, original single-gold misses, false-premise
  reader use, runtime/retry cost and independent confirmation remain explicit debt.

See [final report](../../../../docs/evals/MEMORY_QUALITY_REPAIRS_2026-11-26.md)
and `final-delivery-audit.json` for metric boundaries and source identity checks.
