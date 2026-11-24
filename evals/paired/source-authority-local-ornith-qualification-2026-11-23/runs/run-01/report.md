# Ornith 1.5 local source-authority qualification — run 01

**Decision: NO-GO for this registered model/adapter configuration.** This is a
workflow feasibility screen, not a comparison of memory implementations and
not evidence of an agent-memory benefit.

## Capture integrity

- The four preregistered synthetic families completed in both arms, from
  `2026-11-23T14:13:21.113422Z` to `2026-11-23T14:15:01.802658Z` (100.689 s).
- Both arms used the same `memory_mcp` binary, product configuration, public
  tool schema, prompts, local model alias, adapter parameters and budgets. The
  registered difference was model-managed memory formation versus operator
  file import followed by read-only recall.
- The endpoint was numeric loopback. No API key, bearer token, Codex process,
  external model endpoint, network fetch or real user data was used. Raw model
  response text is not retained.
- The frozen registration checksum verifies, and `compare-agent-managed`
  accepted the run artifacts. See [`SHA256SUMS`](SHA256SUMS) for output hashes.
  Verify them from the qualification directory with
  `shasum -a 256 -c runs/run-01/SHA256SUMS`.
- The model weight hash and LM Studio server build were unavailable. This
  identifies the observed alias during one invocation, not an immutable model
  build.

## Frozen outcomes

| Measure | Incumbent | Candidate | Registered criterion |
|---|---:|---:|---|
| Task-oracle success | 0/4 | 0/4 | At least 3/4 in each arm — **fail** |
| Independent fixed-query probe | 1/4 | 3/4 | 4/4 in each arm — **fail** |
| Candidate file-import probes | — | 2/2 | At least 1/2 — pass, but does not rescue the screen |
| Agent-observed direct-memory success | 1/4 | 3/4 | Descriptive only |
| Hard violations | 0 | 0 | Zero — pass |
| Formation phase status | 1 completed, 3 `model_failure` | 1 completed, 3 `model_failure` | Validity diagnostic |

Task success is zero in both arms. The candidate's direct-memory/probe count is
higher on these four families, but this screen compares two workflows using the
same product binary. It does not isolate a memory change, and four families are
too few to support an agent-benefit claim. The offline paired task-success
difference is 0; the direct-memory difference is +0.5. Neither estimate is an
acceptance result.

The capture's 100.689-second elapsed time is a single end-to-end observation,
not a controlled speed benchmark. It cannot be compared causally with the LFM
or Qwen captures, which used different cohorts and failure/turn mixes.

## Configuration limits and decision

The frozen adapter used temperature 0 and left `top_p` unset. The model card
recommends temperature 0.6 and `top_p` 0.95 for general tasks and documents a
Qwen3 XML tool-call parser plus a Qwen3 reasoning parser for its serving
recipes. The LM Studio setup used here was not independently shown to match
those parsing details. These differences limit the conclusion to this tested
configuration; they do not prove that every Ornith serving configuration is
unsuitable. See the [Ornith model card](https://huggingface.co/ornith-ai/Ornith-1.5-35B-A3B).

The preregistered screen fails because task-oracle success is below 3/4 in both
arms and independent probes are below 4/4. Do not scale this configuration or
reuse these exposed families. A different sampler/parser setup would require a
new adapter contract, frozen settings and disjoint cohort. For faster memory
iteration, keep deterministic profile evaluation as the inner loop. Keep LFM
for fast transport/tool-call smoke only; do not count its quick failed receipts
as useful agent-task throughput. Agent acceptance still requires a model and
configuration that first pass the frozen task/probe screen.
