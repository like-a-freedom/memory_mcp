# Eval Dataset Strategy

> 2026-04-07 · Sprint 0 research note

## Recommendation

Do **not** hard-bind external eval ingestion to legacy dataset variants that were previously adapted to an older `memory_mcp` contract.

Instead:

1. keep each benchmark in its **source format** under a raw dataset area,
2. define one **canonical internal eval-case schema** for this repository,
3. add thin **dataset-specific adapters** that normalize source records into the canonical schema.

This keeps the repository aligned with public benchmarks while avoiding repeated rework whenever the internal MCP response shape evolves.

## Why this is the safer direction

### 1. External projects use source datasets, not one frozen transformed format

Evidence from related resources:

- `MemOS` evaluation explicitly asks users to download benchmark datasets in their source form:
  - `longmemeval_s` from `xiaowu0162/longmemeval-cleaned`
  - `filtered_inter_turns.json` from `amazon-science/PrefEval`
  - `questions_32k.csv` + `shared_contexts_32k.jsonl` from `bowen-upenn/PersonaMem`
  - LoCoMo via dataset URL in the benchmark config
- `Zep` keeps benchmark-specific harnesses for `longmemeval` and `locomo`, rather than forcing one shared transformed dataset shape.

### 2. The benchmark landscape is broader than two datasets

`Awesome-AI-Memory` catalogs multiple benchmark families relevant to long-term memory systems:

- Long-term memory evaluation: `LONGMEMEVAL`, `LOCOMO`, `LOCCO`, `RealMem`, `CloneMem`, `MADial-Bench`, `StoryBench`
- Comprehensive evaluation: `MemoryAgentBench`, `LifelongAgentBench`, `StreamBench`
- Personalization and preference evaluation: `PersonaMem`, `PersonaMem-v2`, `PrefEval`, `KnowMe-Bench`, `MemDaily`

This strongly suggests that `memory_mcp` should treat benchmark ingestion as a **registry of adapters**, not a one-off converter for two already-mutated local copies.

### 3. `longmemeval-cleaned` is already a practical source artifact

The Hugging Face dataset `xiaowu0162/longmemeval-cleaned` states that it **replaces the original LongMemEval dataset** and removes noisy history sessions that interfere with answer correctness.

That means there are really three layers to keep distinct:

1. original benchmark concept,
2. public source artifact actually used by current ecosystems,
3. our internal normalized eval format.

We should normalize from layer 2 -> layer 3, not from an old private layer 3.5.

## Source-verified dataset candidates

## Strong matches for the current plan

### LongMemEval / longmemeval-cleaned

Why it matches:

- cross-session retrieval
- temporal recall
- long-memory QA
- already referenced by the plan
- used by MemOS and Zep benchmark harnesses

Recommended use:

- primary public retrieval benchmark
- source for `tier=direct`, `tier=temporal`, and `tier=reasoning` slices

### LoCoMo / LOCOMO

Why it matches:

- public benchmark
- multi-session conversations
- temporal reasoning
- cross-session recall
- benchmark harnesses already expose latency and category breakdowns

Recommended use:

- secondary public retrieval benchmark
- especially useful for temporal and cross-session tests

### PersonaMem

Why it matches:

- evolving user profile over multiple sessions
- explicit support for preference change over time
- 7 question types including latest preference and preference evolution

Implementation note in this repository:

- sample fixture is still a tiny paired excerpt (`questions_32k.csv` row + matching `shared_contexts_32k.jsonl` slice)
- full official runs use the ignored cache under `tests/fixtures/evals/full/personamem/`
- official `questions_32k.csv` mixes strict JSON `all_options` rows with Python-style / mixed-quote list literals, so the loader must accept both encodings before mapping `(a)/(b)/(c)` labels back to option text

Recommended use:

- personalization / dynamic memory benchmark track
- strong candidate for future `experience` / evolving-profile evaluation

### PrefEval

Why it matches partially:

- official benchmark repo with documented JSON data format
- explicit + implicit preference following
- inter-turn preference retention and persona-driven conversations

Recommended use:

- secondary benchmark for preference-following and implicit memory
- less direct for pure retrieval-at-k, but good for personalized memory slices

## Useful, but not benchmark sources

### Zep eval harness

Useful for:

- harness design ideas
- synthetic multi-source eval shape (`users`, `conversations`, `telemetry`, `documents`, `test_cases`)
- context completeness vs answer accuracy split

Not ideal as:

- public benchmark score source

### qwe-qwe

Useful for:

- memory architecture inspiration
- experience-learning ideas
- hybrid search / graph layering ideas

Not a benchmark source.

### graphify

Useful for:

- graph labeling ideas such as `EXTRACTED`, `INFERRED`, `AMBIGUOUS`
- graph-report / wiki ideas

Not a benchmark source.

### Papers `2511.18423` and `2512.24601`

Useful for:

- design inspiration
- long-context / agentic memory framing

Not dataset sources in the fetched material.

## Proposed repository shape

```text
tests/fixtures/evals/
├── raw/
│   ├── longmemeval/
│   ├── locomo/
│   ├── prefeval/
│   └── personamem/
├── normalized/
│   ├── retrieval_longmemeval.json
│   ├── retrieval_locomo.json
│   ├── preference_prefeval.json
│   └── profile_personamem.json
└── retrieval_cases.json
```

## Proposed pipeline

1. **Acquire source dataset**
   - store raw file(s)
   - record source URL, version, and checksum
2. **Normalize via adapter**
   - benchmark-specific parser
   - output canonical `memory_mcp` eval-case schema
3. **Slice by capability**
   - `direct`
   - `alias`
   - `temporal`
   - `graph`
   - `reasoning`
   - later: `profile`, `experience`, `contradiction`
4. **Run deterministic evals**
   - avoid LLM-as-judge in the core CI path
   - prefer exact/containment/retrieval assertions first

## Implementation note

Current local raw fixtures in this repository are now **source-derived trimmed excerpts**, not synthetic stand-ins:

- `LongMemEval-cleaned` fixture is a trimmed excerpt from the official cleaned dataset record.
- `LoCoMo` fixture is a trimmed excerpt from the official `locomo10.json` sample.
- `PersonaMem` fixture pairs an official benchmark question row with the matching official `shared_contexts_32k.jsonl` context excerpt.
- `PrefEval` fixture is a trimmed official retrieval-track excerpt and is normalized as **user-side preference memory**, i.e. assistant replies are excluded from the retrieval facts because they are recommendation noise rather than stored user preference evidence.

The repository now also makes this explicit in two places:

- `tests/fixtures/evals/raw/README.md` explains that these files are intentionally tiny smoke fixtures rather than vendored full benchmark corpora.
- `tests/eval_external_provenance.rs` provides an ignored, reproducible upstream check that fetches the official source artifacts and verifies that each local fixture is a real trimmed excerpt.

For full official runs, the repository also keeps an ignored cache under `tests/fixtures/evals/full/` and builds adapter-specific bundles on top of the upstream artifacts instead of checking the full corpora into git.

## Suggested next implementation order

1. define the canonical normalized schema for external evals
2. add a dataset registry / adapter abstraction
3. implement `LongMemEval-cleaned -> canonical`
4. implement `LoCoMo -> canonical`
5. add `PersonaMem` and `PrefEval` as secondary tracks
6. only then decide whether `MemoryAgentBench` still merits inclusion

## Bottom line

The current evidence supports the hypothesis that we should **return to source-oriented datasets** and avoid encoding old `memory_mcp`-specific assumptions into a rigid converter.

The right abstraction is not “one script for two already-adapted files”, but “one canonical schema + many thin source adapters”.
