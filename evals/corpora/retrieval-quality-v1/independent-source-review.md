# Phase 1 — independent source-only review

Scope and independence: reviewed the active corpus, sources, label-review, README and pre-prediction selection; selectively inspected the pinned raw LoCoMo dialogue records. No prediction artifact, per-case output or implementation code was inspected before these conclusions. The input hashes and exact binding corrections are in `phase-1-source-only-corrections.json`. No repository file, index or HEAD was changed. This is phase 1 only; branch review awaits the parent's final package.

## Verdict

The populations are a defensible **small native retrieval regression subset**, with real continuous conversation text and realistic minimal project/unknown/ACL/history scenarios. They are not yet a sound precision oracle: blanket “everything outside the minimal upstream citation is prohibited” rejects genuinely useful evidence, and some required groups permit incomplete support. Apply the source-only binding/group corrections before interpreting a strict pass/fail. Do not drop cases, lower thresholds or whitelist whole useful episodes' extracted facts indiscriminately.

The attached JSON is deliberately binding-specific. In particular, `logging-original` may use the HTTP plan's DEBUG/configuration binding and raw episode, while its isolated histogram-only binding stays prohibited. A raw episode can contain useful material mixed with some irrelevant text; that does not make every derived fact useful.

## Verified input population

- Pinned raw SHA-256 is `79fa87e90f04081343b8c8debecb80a9a6842b76a7aa537dc9fdf651ea698ff4`; size is exactly 2,805,274 bytes.
- All 19 public cases contain **every text turn, in order**, from sessions 1 and 2 of their conversation. Each `speaker + ': ' + text` matches raw content exactly. No conversation turn was omitted based on the query.
- The ten distinct conversation populations contain 450 text turns total. Sizes are 35, 44, 44, 51, 39, 50, 58, 50, 39 and 40. The source bundle has 976 case-scoped rows across 35 cases, not 976 independent examples. Reusing a conversation for two questions and the nine project snippets for twelve questions creates correlated cases.
- `sources.json` exactly matches every active case's source list. Selection and label-review preserve upstream questions/answers/evidence. The corrected snake case no longer requires the purchase-date turn as answer support.
- “Complete sessions” currently means complete **text turns**. Upstream image URLs, captions and image-search queries are omitted. That matters for two selected questions below.

## Required support and allowed supplementary evidence for every case

In this table `F(S)` means the specific existing content binding(s) of source S plus its raw episode as listed in the active corpus. Exact new keys, content and rationale are in the JSON. Unless expressly allowed here/there or already required, other bindings remain prohibited/unusable for that query. This includes other people's traits, broad topic overlap, generic encouragement, unrelated task numbers/URLs, denied sources and transaction-future evidence. “Useful supplementary” does not mean sufficient to satisfy required support.

| Case | Required support | Useful allowed supplementary context / ruling |
|---|---|---|
| http-task-16 | Both the histogram rule and DEBUG/log field/rate-limit rule; replace one OR group with two groups, raw:http-plan satisfies both | Keep scope to this task. No supplied source supports worktree status, progress or review rulings; the result can only be a partial context answer. |
| http-task-short | Same two rule groups | Same task scope; Task 16 in Trackly is prohibited. |
| http-task-paraphrase | Same two groups, since both measurements and diagnostics are asked | Neither rule alone proves all requested requirements. |
| logging-original | Existing sink, level-policy and configuration groups | Allow correlation-id formatter contract and only the HTTP plan's concrete DEBUG/configuration binding plus raw episode. Histogram-only fact is still irrelevant. |
| logging-sink | Existing sink group | Allow config binding/episode because MEMORY_LOG_FILE and blank/unset semantics directly determine destination. |
| logging-levels | Existing level-policy group | No extra passages are needed/useful enough. RUST_LOG controls emission/filtering, not MemoryError classification. |
| logging-config | Existing configuration group | Allow sink binding/episode: format and colour are frozen when subscriber installs at startup. |
| logging-correlation | Existing request-id group | Preserve. Generic sinks and log levels do not supply the correlation relation. |
| logging-russian | Existing level-policy group | Preserve same relevance as the English error-policy query. |
| unknown-database | No support exists | Expect empty; all supplied material irrelevant. |
| unknown-url | No production hostname supplied | Expect empty; RUST_LOG/HTTP diagnostics and unrelated example.org URL do not answer. |
| unknown-task | No Task 99 support exists | Expect empty; Task 16 and unrelated numbered sources remain prohibited. |
| locomo-conv-26-082 | Either D2:1 or D2:2 states charity race for mental health; extend the same required OR group | D2:1 is direct evidence currently mislabeled prohibited. Generic self-care and Caroline's mental-health career plans are not the race's purpose. |
| locomo-conv-26-000 | D1:3 and its session timestamp establish yesterday, 7 May 2023 | Other support-group feelings do not constrain the date. Preserve. |
| locomo-conv-30-002 | D1:6 for Jon and D1:7 for Gina; preserve both groups | Allow D2:11/D2:12, which directly corroborate dance as destressing/venting. General dance styles, performances, studio flooring and business updates remain irrelevant. |
| locomo-conv-30-000 | D1:2 and session timestamp establish yesterday, 19 January 2023 | Gina losing a different job this month is prohibited. Dance-studio plans do not establish the loss date. |
| locomo-conv-41-065 | Keep explicit D1:8 for the two main focuses | Allow D1:6 political frame, D1:10 funding priorities, D1:12 specific school improvements, D2:4/D2:5 explicit education focus. Generic encouragement and unrelated family/hobbies stay prohibited. |
| locomo-conv-41-001 | D2:1 and session timestamp establish yesterday, 21 December 2022 | Volunteering alone does not establish when the car was donated. Preserve. |
| locomo-conv-42-096 | D1:13 supplies action/sci-fi | Allow D1:17's explicit love of classics as a supplementary movie-type preference. Movie-watching as a generic hobby and Joanna's genres are not interchangeable with Nate's preference. |
| locomo-conv-42-002 | D1:18 supplies around three years before January 2022, but exact title is missing from all input text | Allow D1:16 as antecedent/context, but repair/document the missing title evidence before claiming complete grounded support; details below. |
| locomo-conv-43-071 | D1:5 team identity AND D1:3 or D1:4 connecting the bare answer to signing | Allow both relation passages. A session timestamp on a bare team name does not itself prove a signing event. Player position, performance goals and endorsements do not answer the asked identity/date. |
| locomo-conv-44-061 | D1:16 explicitly names eagles as mesmerizing | Preserve. Birds in general, hiking spots and future dog adoption do not identify the specific type. |
| locomo-conv-44-000 | D1:7 provides three names/duration AND D1:3 supplies the dogs antecedent | Allow D2:1 to distinguish a later fourth puppy from the first three. Approximate ownership duration supports an approximate year, not an exact adoption date. |
| locomo-conv-47-067 | D1:8 supplies Python/C++ | Allow D1:10, explicit negative evidence that he has used no other languages. John's HTML/CSS are prohibited; James inviting John to help is not James's own HTML experience. |
| locomo-conv-47-001 | D1:26 plus timestamp establishes bowling on March 16 | Preserve exact duplicate content bindings as alternatives. Other games/hobbies and undated slot-machine experience do not meet the dated event. |
| locomo-conv-48-015 | Preserve D2:20 and D2:22 to cover both pets described, Susie and Seraphim | D2:24's purchase date/place is not required and does not answer pet type/name. Generic comfort and gaming details remain prohibited. |
| locomo-conv-48-000 | D1:2 is the only relevant project evidence | It says finished last week relative to Jan 23, not explicitly worked at the beginning of January. Mark upstream temporal ambiguity; details below. |
| locomo-conv-49-083 | Keep D1:2 for new Prius | Allow D1:4: breakdown/repair/sale of the old Prius constrains which car the answer refers to. Its Rockies clause does not independently license unrelated trip facts. |
| locomo-conv-49-002 | Preserve D1:2 family-trip context + D1:4 Rockies + D2:1 Jasper | No additions required. Hiking enjoyment, a cabin atmosphere and Sam's trips do not identify Evan's destinations. |
| locomo-conv-50-071 | D1:15 for few months | Allow D1:9 next-month departure, a constraint distinguishing future planned stay from ongoing duration. Cultural interests and music plans do not determine length. |
| locomo-conv-50-001 | Existing D1:3 mansion + D2:1 car are useful, but support only a new mansion/new luxury car at text level | Allow D1:11 acquisition provenance; D2:5 and D2:17 corroborate vehicle acquisition. Exact Ferrari model/Japan location require missing metadata; details below. |
| historical-observed-cutoff | before only | after is prohibited at the snapshot regardless of semantic match. Phase 2 must verify snapshot_after_source really captures transaction visibility. |
| policy-public-evidence | public only | internal is prohibited even if topically useful; denied provenance cannot supply the public answer. |
| policy-denied-abstention | No accessible evidence | Expect empty, including no internally derived reconciliation/cache/provenance content. |
| qualified-other-project | Trackly task only | HTTP Memory MCP task is prohibited despite matching Task 16 and similar task phrasing. |

## Important source/label defects beyond permitted-binding corrections

1. **Question support is lost when image metadata is stripped.** For `locomo-conv-42-002`, no input text names Eternal Sunshine of the Spotless Mind. D1:16 says only a romantic drama about memory/relationships; D1:18 says “that movie.” Raw D1:16 and D1:18 image-search `query` metadata names the film, but raw `blip_caption` alone does not. For `locomo-conv-50-001`, input text never says Ferrari 488 GTB and does not explicitly establish the mansion's Japan location. Raw D2:5 image-search `query` is `ferrari 488 gtb japanese mansion`; raw D1:3 `query` is `mansion entrance japan`. Additionally, “my new mansion” and “my agent found me this awesome place” do not explicitly distinguish purchasing from arranging accommodation; the upstream purchase wording is stronger than the dialogue. This is a **source coverage limit**, not retrieval evidence against the model. Concrete repair: preserve source-labeled image metadata uniformly for all raw image-bearing turns (not selected by answers), regenerate native preparation/bindings and identities, and describe the corpus as text plus upstream image metadata. Do not relabel search metadata as a verbatim user assertion, and do not add answer text as source content. If text-only ingestion is deliberate, retain these cases but explicitly limit what their retrieved-support scores prove; they cannot demonstrate title-grounded or model-specific answer correctness.

2. **Upstream date question overclaims its source.** `locomo-conv-48-000` asks about the beginning of January 2023; D1:2 at Jan 23 says the project finished last week (Jan 16–22 under the ordinary calendar interpretation) and gives no start date/duration. Electrical engineering is relevant evidence, but presence at the beginning of the month is unproven. Record an `upstream_temporal_underspecified` annotation while preserving case, original query, source and selection. The native gate may measure retrieval of relevant project context, but must not count this as proof of correct event-date reasoning. A separate source-grounded formulation can be added (“What kind of project did Jolene finish in the week before 23 January 2023?”) without replacing or dropping the original case.

3. **Broad Task 16 query asks for unavailable status/rulings.** Its only target excerpt contains design requirements, no execution status, current worktree or reviewer rulings. This is a useful retrieval regression for qualified task selection; do not describe it as answering the whole original question. Preserve it with a `partial_source_coverage` annotation, or add independently sourced status/ruling passages. Do not infer completed work from an approved plan.

4. **Required-group incompleteness.** All three HTTP queries currently accept either histogram or logs as fully recalled; they need two groups. The signing and dogs cases need the predicate/antecedent relation as specified above. This strengthens support completeness; no threshold reduction is proposed. Conversely the race participant's direct statement is a real alternative to Caroline's reply and must not be a mandatory false negative.

## Honest scope/coverage limits (not a demand to expand this subset indefinitely)

- Complete two-session populations make this materially more realistic than seeded gold facts and selected distractors. They still cover short histories with 35–58 turns, not full LoCoMo, long-term consolidation, high-density mixed-project stores, or official benchmark scoring.
- Project snippets are controlled/sanitized examples, and twelve queries share the same nine snippets. ACL and transaction-history cases are small observable contracts, not stress tests. Source-only inspection cannot establish reconciliation/cache isolation; that belongs to phase 2 and the production tests.
- The active cases all say `split: test`, but if these exact questions or predictions drove implementation/threshold choice, report the final result as a development regression gate rather than an untouched estimate of generalization. Independent source-only labels protect the oracle from prediction-driven relabeling, not from implementation overfitting to known queries.
- Categories are preserved upstream. Calling category 4 uniformly “single-hop” is misleading for the Jon/Gina case, which needs evidence for two people. “First eligible category-4 and category-1/2 questions” is more exact.
- A binary allowed/prohibited disposition conflates irrelevant-but-public with security/temporal-denied. Keep distinct tags/diagnostics so a precision error and a disclosure violation are never presented as the same severity.

## Declined in phase 1

All code behavior; model scores/ranking; measured precision/recall; whether labels match actual output identities after any forthcoming extractor changes; runtime/memory limits; cache/provenance leakage; threshold calibration; final corpus-vs-baseline comparability. Those are intentionally pending phase 2. No retrieval prediction artifact has been consulted.

## Calendar and minimal-budget extension

The extension in `independent-source-review-extension.json` was adjudicated from the two authored source excerpts and the original LoCoMo turns before retrieval predictions for the new cases. For “What changed in Atlas during 2026 Q2?”, the Atlas Q2 persisted fact or its source episode is required; the Trackly Q1 material is prohibited. For “Show the timeline of Atlas changes during Q1 2026.”, both source periods are explicit but neither jointly supports Atlas/Q1, so the correct outcome is abstention. The two notes are authored, sanitized regression inputs, not external benchmark evidence.

The existing stdio sink question retains its source-reviewed evidence from the logging-sink note and source episode, with the unchanged complete nine-source case population. Its response budget is set to one to exercise the native minimal-budget contract without changing the query or relevance labels.
