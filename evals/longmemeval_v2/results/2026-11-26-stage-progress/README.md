# Partial source evidence and lexical work reuse — 26 November 2026

Base: `54877d2c`. Local Apple Silicon only; no LM Studio, inference, model answers,
new dependencies or migrations. This continues the committed narrow-FTS archive;
that archive is unchanged. Task 7 full-history acceptance remains **open**.

## What changed and what the tests establish

- Caller-owned, opt-in `ContextEvidenceCapture` retains completed source-linked
  boundaries after cancellation or storage error. Four public controlled-adapter
  cases cover interruption, original error preservation, access exclusion and
  rejection of reusing one capture across reads. Existing convenience API stays.
- Episode fallback computes one lexical score per candidate before stable sorting;
  response confidence and grounding reuse one score. Candidate admission, limits,
  tie-breaks and score arithmetic stay unchanged.
- Sentence normalization streams across the complete text and skips only later
  identical raw sentences, which cannot replace their first identical winner.
  Global quote/reference preprocessing still guards empty vocabulary. The original
  ordered/count-valued tokenizer remains unchanged. Two literal controls and a
  128-case fixed-seed independent old-traversal scalar-score oracle pass; this is
  not an exhaustive proof of all ranking behavior.
- The fixed 16-source real-store public read preserved the literal five source
  IDs, confidence and grounding. Caller-thread allocation calls fell from
  **534,786** (valid runtime RED) to **114,542** (matched default-feature final run),
  below the original 200,000 bound. Opt-in run observed114,546. This counter covers
  the calling thread during the public read, not allocations on every database
  worker, allocated bytes, peak memory, or end-to-end latency.

`tdd/` retains every intermediate failure. Missing-API compiler errors and invalid
fixture visibility attempts are setup, not behavioral REDs. A temporary
future-owned-recorder mutation reproduced lost completed state; the capture-reuse
check separately failed at runtime before its guard. Cache-only471,370 and
sentence-reuse219,007 remained RED; response reuse176,206 passed before the final
streaming/presence change. Neither resource thresholds nor source limits were
relaxed.

## All full-store attempts, including failures

The frozen100 trajectories /151 episode segments /six snapshot files were not
reformed or reduced. Each process reads a disposable complete copy; registrations
precede execution, sources/binaries are hashed and original snapshot hashes are
checked after each attempt.

| Capture | Cases | Bound, cooperative /process | Completed contexts | Outcome |
| --- | ---: | --- | ---: | --- |
| root partial diagnostic | dropdown only |120/150s |0/1 | read121.192s; process135.638s, exit101 |
| `candidate-read/` | dropdown, toolbar |30/45s |0/2 | external kill45.146/45.116s |
| `streaming-read/` | dropdown, toolbar |30/45s |0/2 | external kill45.187/45.116s |

These are **five development process attempts on two already exposed cases**, not
five independent quality examples. All five fail to produce a complete context.
The narrow-FTS archive separately retains15 earlier attempts; no successful-only
selection or claim of independent replication is made.

The root partial diagnostic observes the reviewed dropdown source span at
formation0.510ms, ranked-fact candidates86,146.143ms and episode
candidates114,305.153ms. These are cumulative offsets, not isolated costs.
Selection and packing remain unknown. Its CPU sample failed under sandbox; the
raw command failure remains. Later owned-process native samples succeeded.

At20s the final dropdown CPU sample is in application lexical candidate metrics;
the toolbar sample also shows SQL/SurrealDB execution and value cloning. A
point-in-time CPU sample does not assign all time to one function. The two final
processes yielded no partial receipt before SIGKILL. Caller-owned in-process
state survives cooperative cancellation/errors, **not external process death**.
A30s Tokio timeout is not a hard CPU bound; the external owner still kills/waits.

## Interpretation and next decisive check

The equivalent small-control resource repair is verified. Full-history evidence
acceptance is still0/2; final-context recall and packaging are unobserved, not
measured zero. The seven unchanged archived answers remain2/7; no new reader or
judge ran. Old C1 remains stopped; C2 is not admitted.

Next, collect bounded metadata for the actual lexical candidate pools before more
optimization: record count, total/max content and index-key bytes, unique exact
content/source counts, channel completion and work per record. Persist only
counts/digests at boundaries so process termination preserves diagnostic progress.
Use the fixed same-store query/time/source contract. Check SQL plans for payload
hydration and graph work, separately from application normalization.

If exact repeated content is measured, test request-local content-feature reuse
or same-statement deduplicated payload fetch against the independent ordered SQL
and lexical oracles. Do not change candidate caps, drop terms, truncate reviewed
blocks or bypass search with a known source. Require the original30s full-context
2/2 and current gate8/8 before seven-answer reevaluation. No end-to-end latency
claim can be inferred from allocator counts alone.

Fresh verification, immutable native gate, commands and source identities are in
`verification.json` and `native-gate/`. `manifest.json` hashes the retained files.

Staged source/document whitespace checks exclude raw `.log`, `.txt` CPU dumps
and `.patch` receipts. Unified patches contain required single-space context
lines for blank source lines; these are kept byte-exact, not rewritten to make
an artifact-as-text whitespace check pass. The first inclusive check and its
failure are recorded in the session; source/document checks remain strict.
