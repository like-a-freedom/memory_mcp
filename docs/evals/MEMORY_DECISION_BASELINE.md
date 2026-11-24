# Memory decision baseline on master

> **Historical default note (2026-11-21):** The frozen runs below correctly
> describe the then-default `shadow` behavior. Product default is now
> `evidence`; the same-binary public-MCP comparison is recorded in
> [the current rollout replication](../../evals/paired/claim-rollout-current-master-2026-11-21/README.md).
> This later result does not rewrite the original baseline or establish a
> population-level or model-agent benefit.

**State:** the starting-revision deterministic baseline below is frozen and unchanged. The later temporal candidate implementation and its v2 development runs are recorded in [Post-change temporal development evaluation](#post-change-temporal-development-evaluation); those runs do not replace the baseline or establish an agent-usability benefit.

> **Oracle scope correction (2026-11-23):** The historical v9 oracle uses
> `memory-decisions-oracle/v1` and expects `current` on three ordinary
> non-snapshot reads. ADR-0083 promises temporal roles only on revision-visible
> snapshot reads, so those v9 `decision_unobservable` results are not evidence
> that memory returned the wrong fact. The v9 artifacts remain untouched as
> historical diagnostics. New oracle v2 corpora reject role expectations
> unless their probes select an exact temporal snapshot; see the v10 baseline
> below.

## Frozen target and execution setup

- Product revision: `1ae1da4a4e3120f2de94753fc1a949caee3aced7` (`v1.41.0`). The evaluation worktree was on that revision with uncommitted harness changes.
- Product executable: `/Users/solovey/Documents/dev/memory_mcp/target/debug/memory_mcp`; SHA-256 `f01fc63c1acc8558ca1c6e7b5aa01a1fdf478d95e6d09effcc6ff38add6c050a`.
- Evaluation executable: `/Users/solovey/Documents/dev/memory_mcp/target/debug/memory-eval`; SHA-256 `a088451a8ff67174fe065b66a799b8282227572fe89cff7cb2d0d397aaf02e28`. `Cargo.lock` SHA-256: `d5879428097dfb38ae0eda3470964fdd5331e5747f6b2d6d50e7e806f530f5d3`.
- Runtime: macOS/aarch64, `rustc 1.100.0`, debug binary, no optional Cargo features.
- At this frozen baseline capture, the primary path was the deployed default `claim_rollout_stage=shadow`; a separate fresh-store `evidence` rollout read was diagnostic only. Each probe also had a separate verbose read where supported.
- Provider profile: embedded local SurrealDB, namespace `main`, embeddings disabled, lexical/episode fallback retrieval, regex NER, transaction time assigned by the server. No caller-supplied `t_ingested` is used.
- Corpus SHA-256: `b7d42564f692d7515c06f08f404b627207c3d7c9f34ba474ac99a4dd0e44b8b0`. Input-schema SHA-256: `bbdbae2c07912a2c09a1d27a5998ec56e0e08ffe564164a559aac9c5ddf41877`. Profile/config SHA-256: `3144a7d443da339afa86af571d95cc486ae959fa28c61e2610598194fdbcc483`.

The ten probe IDs and oracle were fixed before the accepted runs. Each probe used public MCP `ingest`, `extract`, `assemble_context`, and provenance `explain` calls. Histories ran in isolated stores. The runner recorded raw responses, extraction results, bindings from returned episode IDs, and immutable per-case receipts.

## Reproduction and repeatability

Run from the repository root, changing both output paths for the second execution:

```sh
MEMORY_DECISIONS_SERVER_BIN="$PWD/target/debug/memory_mcp" \
MEMORY_DECISIONS_PRODUCT_REVISION=1ae1da4a4e3120f2de94753fc1a949caee3aced7 \
MEMORY_DECISIONS_ARTIFACT_ROOT=/tmp/memory-decisions-master-final-c \
cargo run -p eval-harness --bin memory-eval --locked -- run \
  --profile evals/profiles/memory_decisions.json \
  --artifact /tmp/memory-decisions-master-final-c.json
```

| Run | Run ID / start (UTC) | Verdict | Coverage and metrics | Report SHA-256 | Receipt index SHA-256 |
| --- | --- | --- | --- | --- | --- |
| C | `run-1795202571` / `2026-11-20T19:22:51Z` | Quality failed; budget passed | 10/10, 1 passed, 9 quality failures, 0 invalid; evidence 0.60, decision 0.10, probe 0.10 | `ac494b1ed4b0507303c54a46251779436265f5944ec91b0c7e770f1f45d9327b` | `fed09d785257981409f1bba36de498ce440c9a1738a6145d2e5949e56aa69587` |
| D | `run-1795202776` / `2026-11-20T19:26:16Z` | Quality failed; budget passed | 10/10, 1 passed, 9 quality failures, 0 invalid; evidence 0.60, decision 0.10, probe 0.10 | `f57f3b4eea28ac52b4153b884aa78c2cd85c38a702b9d5d16196f87849771481` | `f43bad7d07ed81c4013e3a0b862081b8c90a077f376d1177be329c9f8b2c2608` |

Artifacts and per-probe receipts are in `/tmp/memory-decisions-master-final-{c,d}.json` and `/tmp/memory-decisions-master-final-{c,d}/`. Their receipt indexes carry the pinned product, binary, corpus, config, and schema identities above. The replay comparator passed on C/D: all ten cases matched on source order, exposed roles, disposition, three scores, and reason codes. It intentionally ignores run-specific receipt paths/hashes and generated IDs.

## Case-level result

`evidence` means the observed public source content covers a complete allowed evidence set and violates no forbidden-source rule. `decision` requires an observable expected disposition/role. `probe` is their conjunction. `unknown` in the roles column means the public response did not expose a role; it does not mean that the source was treated as historical or conflicting.

| Probe | Oracle | Default compact read | Evidence / decision / probe | Main reason |
| --- | --- | --- | --- | --- |
| `late-before` | North from the initial source, current | Initial North source only; role unknown | 1 / 0 / 0 | `decision_unobservable` |
| `late-after` | South from the correction, current; earlier North may be historical | Initial North source only; role unknown | 0 / 0 / 0 | `missing_evidence`, `unsupported_temporal`, `decision_unobservable` |
| `late-retrospective` | North as known before the correction; correction excluded | Correction source only; role unknown | 0 / 0 / 0 | `forbidden_source`, `missing_evidence`, `unsupported_temporal`, `decision_unobservable` |
| `ordinary-update` | North for May 1; May 9 update excluded | May 9 update only; role unknown | 0 / 0 / 0 | `forbidden_source`, `missing_evidence`, `unsupported_temporal`, `decision_unobservable` |
| `conflict-open` | Both peer assertions, explicitly unresolved/conflicting | Both sources returned; both roles unknown; generic `supported` disposition | 1 / 0 / 0 | `decision_unobservable` |
| `conflict-reversed` | Same unresolved result after reversing arrival order | Both sources returned; both roles unknown; generic `supported` disposition | 1 / 0 / 0 | `decision_unobservable` |
| `conflict-resolved` | South from the explicit resolution; old assertion historical | Initial and peer sources returned; resolution absent; roles unknown | 0 / 0 / 0 | `missing_evidence`, `decision_unobservable` |
| `unknown` | No assignment and no unrelated support | Empty result with `unknown` disposition | 1 / 1 / 1 | — |
| `stable-repeat` | West from the single stable source on both reads | Stable source returned; role unknown | 1 / 0 / 0 | `decision_unobservable` |
| `unrelated` | North from its own source; South Depot must not contaminate it | North source only; role unknown | 1 / 0 / 0 | `decision_unobservable` |

The protocol's `supported` status is not treated as proof that an assertion is current, authorized, or conflict-free. Both open-conflict reads return the two assertions, so evidence coverage passes, but the compact public result does not identify them as conflicting. The same output limitation prevents a positive role/decision score for the supported single-source controls.

## What the baseline establishes

1. **Visible source-selection misses:** `late-after` returns the superseded North source while omitting the correction; `conflict-resolved` omits the resolution; and `ordinary-update` selects the May 9 update for a May 1 question. In the retrospective probe, the correction source appears despite the requested pre-correction knowledge boundary. These are direct observations from the pinned public read path; the results identify the failing stage as source selection/output, but do not by themselves isolate its internal ranking cause.
2. **Decision semantics are not exposed by the compact read:** current/historical/conflicting roles and a conflict/correction disposition are absent. The MCP schema also has no independent valid-time and knowledge-time cutoffs. Requests that need both therefore report `unsupported_temporal`; natural-language wording is retained in the traces and is not silently scored as a supported two-cutoff API call.
3. **The default uses episode fallback:** for example, the late-correction receipt returns `fact_id=episode_fallback:…`, with lexical fallback and `semantic_available=false`. The receipt-complete replay below confirms that `extract` projects claims and persists a contradiction relation for the later episode, while the compact read still returns only the old source. The read path therefore does not consume or surface that available relation for this query.
4. **The unknown control works in this fixture:** it abstains with no source and is the sole complete probe success. This is one constructed control, not an estimate of general abstention quality.

The rates are deterministic fixture diagnostics, not estimated population accuracy. In particular, `decision_success_rate=0.10` means only the unknown case met the suite's currently observable decision contract; it must not be restated as “the memory made the right decision 10% of the time.” The 10 cases are development probes, not independent or held-out samples, and they do not measure generated-agent answer quality, long-term utility, or user-perceived convenience. No product improvement is claimed from this baseline.

## Follow-up retrieval-source audit

This is a static code-path audit performed on the same master revision after the scored runs. It is **not** a measured regression and is not included in the ten-case baseline denominator. The current suite does not isolate these selector paths; they need deterministic cases before a temporal implementation can pass its held-out gate.

- `assemble_context` merges lexical, temporal, alias, experience, triple, graph, community, semantic, source-episode, fallback, and source-neighborhood paths in [`pipeline.rs`](../../crates/memory-mcp/src/memory/retrieval/pipeline.rs).
- The experience path selects active facts against current wall-clock time, then applies `fact_is_active_at` at the request cutoff; that helper also checks `t_ingested` and invalidation. This second filter prevents a late-known fact from leaking into an earlier `as_of`. The current-time preselection can instead cause a historical false negative when a fact was active at the requested earlier snapshot but was invalidated later, and can omit known facts whose `valid_at` is in the future. These are source-derived risks, not regressions measured by the baseline receipts.
- Entity alias lookup and community-summary search have no requested-time predicate. Triple lookup selects the current fact-link set before applying temporal visibility to the linked facts. Their effect on an older snapshot is plausible from query shape but was not measured by the baseline.
- Community summary and membership are mutable projections; embeddings on facts can be updated in place, changing ANN candidate discovery; fact access heat is also updated by reads and affects ranking. Historical reads must version these selector inputs or exclude them under a policy that does not claim a complete historical ranking.
- Graph edges, facts, and episode/source-context reads already use a shared cutoff. They still need independent valid-time and committed-knowledge boundaries; the present `as_of` applies one cutoff to both.

This baseline's observed failure attribution is limited to its public probes. Later public regressions and remaining selector boundaries are recorded in the [measured results](DETERMINISTIC_MEMORY_QUALITY.md) and [temporal compatibility contract](../compatibility/TEMPORAL_CONTEXT.md).

## Existing regression control

The existing retrieval-quality profile has 37 development cases and six established 1.0 floors. Its accepted native-model control used the exact local bundle revision directories (each environment path directly contains `config.json`, `tokenizer.json`, and `model.safetensors`):

```sh
EMBEDDINGS_ENABLED=true \
EMBEDDINGS_PROVIDER=local-candle \
EMBEDDINGS_MODEL=intfloat/multilingual-e5-small \
EMBEDDINGS_MODEL_DIR="$PWD/work/evals/models/intfloat--multilingual-e5-small/614241f622f53c4eeff9890bdc4f31cfecc418b3" \
RETRIEVAL_RERANKER_MODEL_DIR="$PWD/work/evals/models/cross-encoder--mmarco-mMiniLMv2-L12-H384-v1/1427fd652930e4ba29e8149678df786c240d8825" \
RETRIEVAL_RERANKER_ADMISSION_FLOOR=0 \
MEMORY_CLAIM_ROLLOUT_STAGE=evidence \
cargo run -p eval-harness --bin memory-eval --locked -- run \
  --profile evals/profiles/retrieval_quality.json \
  --artifact /tmp/memory-decisions-retrieval-control-valid.json
```

Accepted run `run-1795202063` started `2026-11-20T19:14:23Z` and passed 37/37 with 0 invalid. All six established floors remained `1.0`: `precision_macro`, `required_group_recall`, `unknown_abstention_rate`, `answerable_non_abstention_rate`, `duplicate_free_rate`, and `prohibited_free_rate`. Provider status was `ready`; model identity is E5 revision `614241f622f53c4eeff9890bdc4f31cfecc418b3`, 384 dimensions, with the pinned mMARCO reranking signature `reranksig:dff4b9939786ed9a1e897946778e336584cc24251a2f32282bd3fe83a680a7d1`. Artifact: `/tmp/memory-decisions-retrieval-control-valid.json`, SHA-256 `6a1bb92e62dca96f770f9ddcc1583ab29063d47d67ba4748742aeff17c773774`.

Two earlier invocations were invalid preflight attempts and are excluded: `/tmp/memory-decisions-retrieval-control.json` (SHA-256 `a6c7ede85024ee908dc349e73f23ed3de96ab416ac021f891d4a537bc7c2f5df`) and `/tmp/memory-decisions-retrieval-control-corrected.json` (SHA-256 `c4dee67ab9850c97f07c50f7511c361d0731670a8ee0b337cbb2b00d49321046`). The first pointed both model variables at parent cache directories; the second corrected the reranker path but still passed the E5 parent directory. Provider initialization failed before cases ran. The accepted run uses both pinned revision directories and changes no profile, sources, or code.

The profile and corpus remain unchanged. This run checks that adding the decision suite did not disturb the existing retrieval path; it is not an official LoCoMo or LongMemEval score.

## Related baseline evidence and boundaries

- The pinned release workflow on the same product revision reported 154/154 evaluations passed; the existing claim-reconciliation suite passed 41/41, and its relation metrics were 1.0. The original artifact SHA-256 was `3f2205f5c87a1aeda9f1e8f790d753199714ad388232d49886e8fb2a6e888c89` ([workflow run](https://github.com/like-a-freedom/memory_mcp/actions/runs/44755832724)).
- After removing fixture-supplied transaction-time backdating from the eval adapter, local claim-reconciliation also passed 41/41 on this pinned product revision and matched the release artifact per case. That suite tests extraction/relation behavior, not these later public read decisions.
- This baseline ran on one macOS ARM64 machine with embeddings disabled on the decision suite. It does not establish cross-platform repeatability, model-backed semantic retrieval quality, public agent usability, or the benefit of any future memory change.
- `evals/profiles/memory_decisions.json` is a gate-free nightly diagnostic profile. Its quality-failed verdict is expected baseline evidence and must not be wired into blocking PR/release profiles as a passing golden set.

### Agent-loop connectivity smoke (not an accepted pilot)

On 2026-11-20, Codex CLI `0.169.0` ran one synthetic task against an isolated local stdio server and temporary SurrealDB store. The model made one `assemble_context` call, received two episode-fallback items, and answered “North Depot” with the correct returned `source_id` (`pilot/decision-may-01`). The task passed its single-case answer/source check. The CLI needed an invocation-only `approval_policy=on-request` with Auto-review enabled; this did not change the user's persistent config. The successful raw trace is `/private/tmp/memory-mcp-agent-pilot-20261120/events-on-request.jsonl`, SHA-256 `dcd98e3fda594f5735a158891dd16e752719ee54b82b34065a70df27be2f2a5f`.

This is only a transport smoke. It used a manually seeded, single-history store and the CLI's default model, whose exact identity was not present in the trace. It has no paired arm, repeated trials, independent task families or pilot-derived margins. Codex CLI reported 82,586 input tokens for the whole invocation (67,712 cached), which includes its system/tool context and is not a comparable per-task interaction-cost measure. Do not include this run in the deterministic suite metrics, statistical estimates or any usability claim. The fixed-agent pilot and held-out paired evaluation remain open prerequisites.

### Initial ChatGPT-login fixed-agent diagnostics (excluded from pilot variance)

On 2026-11-21, three local invocations used Codex CLI `0.169.0` authenticated with the existing ChatGPT login via `--codex-home /Users/solovey/.codex`; a separate `CODEX_API_KEY` was not needed. Each invocation ran the same frozen incumbent binary in both arms across the same eight development history families. Each arm recorded 1/8 task success and 1/8 direct-memory success. All 16 memory MCP calls per invocation failed with `error_class=unknown` and the same error-content digest `554ead599db593b8ff0210e503c82c2a69e3ffc93fd27af0585bfc383706b64d`. Thus these are 24 repeated per-arm task attempts with zero usable memory-call evidence, not pilot observations suitable for variance, power, margins, a product comparison or a usability claim. The exact Codex tool arguments were not retained in the privacy-preserving receipts.

The three run IDs were `4502-1795259353855331000-0`, `4926-1795259667016056000-0`, and `8436-1795261437757501000-0`. Their metadata-only artifacts are preserved under [`evals/paired/temporal-pilot/results/`](../../evals/paired/temporal-pilot/results/); the third is labeled `diagnostic-run-3`. The descriptive test-retest report is `local-chatgpt-pilot-report-v3.json` (SHA-256 `e9e5674a6a6d4e61f75d7985ac4c4189374281b5c03f3225bdcf6f02dfa227f2`). It shows 1/8 in each run for task and direct-memory success and zero observed test-retest sample variance across the eight family outcomes. That zero is a property of these binary outcomes on eight development families, not evidence of stable agent performance or adequate statistical power.

A separate smoke against the frozen baseline binary confirmed that `assemble_context` advertises `as_of` but not `valid_at`; `valid_at` is rejected as an unknown argument, while a query-only request is accepted. The smoke error digest differs from the first-run approval error, and the actual pilot arguments are unavailable, so it does not establish a parameter-level cause. The approval failure was corrected below; the later valid same-product test-retest is described in the next section.

**Root cause and harness correction (2026-11-21):** A one-call local diagnostic exposed the error text whose SHA-256 was also present in the first three pilot invocations: `MCP tool call requires approval, but approval policy is never`. The failure was in the Codex child configuration: `--ignore-user-config` deliberately omits the user's config, while the runner did not set a per-invocation approval policy. Thus the repeated MCP errors were approval denials; the separate `valid_at` schema smoke was unrelated. The runner now sets `approval_policy="on-request"` and `approvals_reviewer="auto_review"` only for the child invocation. It keeps `--sandbox read-only`, disables shell, apps, plugins, memories, and web search, and exposes only `assemble_context`. The user's persistent Codex configuration is unchanged.

The live JSONL trace also showed that Codex emits `error: null` alongside successful MCP results; the parser had treated the field's presence as failure. A regression test reproduced the false `tool_error` (red before the fix, green after). A second regression ensures that when a Codex wrapper error accompanies an MCP `result.isError`, the MCP result drives classification. A new error class distinguishes approval-policy denials.

The first approval-enabled diagnostic run reached the MCP tool successfully, but its first completed tasks exceeded the pre-existing 32,768-token context budget at 36,640 input tokens. It was stopped before the remaining trials; those budget-exceeded observations are not pilot evidence. The development plan is preserved in a new, separately hashed [`plan-context-65536.json`](../../evals/paired/temporal-pilot/plan-context-65536.json), raising the cap to 65,536 based on that observed usage. The earlier approval-denied and budget-exceeded runs remain excluded from power analysis and promotion decisions.

### Completed same-product test-retest (descriptive development pilot)

Two independent invocations then completed all eight families in both arms using the same frozen incumbent product identity on both sides. Run A (`17112-1795263976666481000-0`, `2026-11-21T12:26:16Z`–`12:33:41Z`) had task-oracle success of 3/8 incumbent and 4/8 candidate; direct-memory success was 4/8 in each arm. Run B (`17815-1795264448916176000-0`, `2026-11-21T12:34:08Z`–`12:42:36Z`) had task-oracle success of 4/8 per arm and direct-memory success of 5/8 per arm. In run B, `assemble_context` returned `invalid_arguments` in three distinct trials per arm; these are retained as agent-visible outcomes, not transport or runner failures. The tool arguments were not persisted, so the exact parameter-level cause is unknown.

The metadata-only report at [`local-chatgpt-pilot-report-context-65536.json`](../../evals/paired/temporal-pilot/results/local-chatgpt-pilot-report-context-65536.json) has SHA-256 `885b62e9fdc9c943b7b936f349aab55b95e5409beb2c0e934b47dd4e99d8d293` and binds canonical plan SHA-256 `2083755f8698167e848a25a365b12175c5f24c949cc0bf7a73a7b7e0fafec91c`. The equal-weight family task-success means were 0.4375 in A and 0.5000 in B (test-retest sample variance of family deltas: 0.2455357); direct-memory means were 0.5000 and 0.6250 (variance: 0.0535714). The eight development families are too few to establish model stability, power, a candidate effect, or general agent usability. This report is descriptive variance input only. A joint-rule power analysis still needs a preregistered smallest worthwhile effect and target alternative; the disjoint confirmatory cohort remains unrun. No memory-quality or usability improvement is claimed from this same-product test-retest.

## Decision gate recorded with the frozen baseline

At the time this starting-revision baseline was captured, the receipt audit selected independent temporal cutoffs plus relation-aware projection for dated reads as the next candidate. The baseline artifacts were preserved before that implementation work began. The candidate is now implemented on the authorized `master` worktree and its in-sample development results are recorded below. Those results do not satisfy the separate fixed-agent pilot, disjoint held-out cohort or paired agent-facing promotion gate, and they do not show a general quality or usability improvement.

## Receipt-complete reviewer replay

On 2026-11-20, the reviewer found that the original evidence-enabled diagnostic stored its read response but dropped that arm's own ingest and extraction receipts. The harness now retains those traces under receipt schema v2. This is an evaluator observability fix only: product binary, corpus, input schema, profile, score logic, and provider configuration are unchanged. The earlier C/D artifacts remain preserved above as the original accepted baseline.

| Run | Run ID / start (UTC) | Verdict | Coverage and metrics | Report SHA-256 | Receipt index SHA-256 |
| --- | --- | --- | --- | --- | --- |
| Review C | `run-1795204976` / `2026-11-20T20:02:56.921303Z` | Quality failed; budget passed | 10/10, 1 passed, 9 quality failures, 0 invalid; evidence 0.60, decision 0.10, probe 0.10 | `08215ab34a51afce34355c7dfacf4f278863c85dd0d7f210749a817b3db6cbb4` | `b599e77cb56ce5e02d98b6fd772342e4af8040c49725ed4cf26b285673aeaf2b` |
| Review D | `run-1795205278` / `2026-11-20T20:07:58.377116Z` | Quality failed; budget passed | 10/10, 1 passed, 9 quality failures, 0 invalid; evidence 0.60, decision 0.10, probe 0.10 | `58f4e45b786798353b94ac8b344900ced94ed4d5c5d8201f5b7e40837d7859c9` | `3d52a61459cee78e6aefe564c8e1a1049e1c64dbea7dec162e09c14da45ed94a` |

Artifacts are `/tmp/memory-decisions-master-review-{c,d}.json` and their sibling receipt directories. The receipt comparator passed. Product executable SHA-256 remained `f01fc63c1acc8558ca1c6e7b5aa01a1fdf478d95e6d09effcc6ff38add6c050a`; the updated evaluator executable is `f3669d80827b37f285c71f0f050643f760584e1455df995126c019c63979f34a`. The product revision, corpus checksum, schema checksum, provider configuration and lockfile match the preceding C/D runs.

The newly retained extraction traces make the source-selection diagnosis more precise:

- `late-after`: extraction projects one claim and records one active `contradiction` relation for the correction episode. The public compact read in both `shadow` and the separately labelled `evidence` run still returns the initial North episode as `episode_fallback`.
- `conflict-open`: extraction records an active contradiction; the read returns both peer episodes, but compact output exposes no typed conflict role.
- `conflict-resolved`: the source text says “explicit correction” and shares the initial event's declared `source_id`, but extraction records two contradictions rather than a correction relation. The source type/id are caller-supplied labels, not authenticated owner identity. This probe tests same-declared-lineage correction wording, not external authorization.
- `ordinary-update`: the later event yields a contradiction relation, while the dated natural-language query returns the May 9 episode for May 1. The tool has no structured valid-time parameter.

The old `decision_success_rate=0.10` should be read as “one of ten compact responses exposed the oracle's typed disposition/roles.” It is an interface-signal metric, not an estimate of an agent's ability to reason from returned source text. A real agent-task loop is still required for any convenience or downstream-success claim.

## Implementation follow-up

The implemented temporal and conflict-aware contract is recorded in [ADR-0083](../adr/0083-revision-bounded-temporal-context.md). The evidence supports a coherent experiment on independent `valid_at`/`known_at` cutoffs and relation-aware projection for dated reads. It does not support retention changes, default rollout promotion, an authenticated-authority feature, or Anthropic-inspired consolidation/indexing in this phase.

## Post-change temporal development evaluation

This section records an in-sample development experiment on the authorized `master` worktree after the frozen starting-revision baseline above. It does not rewrite the starting corpus, oracle, profile, or receipts. The product `HEAD` remained `1ae1da4a4e3120f2de94753fc1a949caee3aced7`; the changed product binary is identified by SHA-256 below, and the working-tree implementation is not represented by that Git commit alone.

The experiment used `memory-decisions-v2` to correct the probe's valid-time boundary from midnight to the source events' exact `10:00 UTC` effective time. Its event, probe, oracle, and profile SHA-256 values are `61704878478dd8e092187fe9ef8d6a2eeba716fc05e46a90582a14c64fae4ed0`, `6e6beaf8f2189b70521d402daa2b8c2721eff186dd3760ea25fb8cfea14cba81`, `c977507ee23aae6b769869b9d4bb73f0f9f197da54005ec85016238ef407106b`, and `29cce488b71d1d218efdde94089ae5fa1839ec65f0d464c0fc6005eb801799d5`. The combined corpus and input-schema hashes recorded in the receipts are `ad5a76dbb3d8e0a1876b1f7c53c8e3c9f94f219d1def2cbb1fff775492201132` and `7b8c60a7d6bdc5d50a71b04e5d2d3dc514516dd69b46627f5b0481cd41c20e61`; config hash `3144a7d443da339afa86af571d95cc486ae959fa28c61e2610598194fdbcc483`. Product binary SHA-256: `8dcc1c148750ba9bca3eec2799f16fbd7f0751229e43b26676599efb45fe79d1`. Evaluation binary SHA-256: `150c032dad46b00c6cc666d357786f5b99c6e0af0afc4613e811c12020459f8a`.

The first pair exposed a real valid-time projection defect: a `contradiction` relation from an update effective May 9 made the May 1 source look conflicting even though the update itself was outside the requested valid-time slice. A public integration regression failed on `Conflicting` versus expected `Current`; relation projection was corrected to require both contradiction participants to be valid and visible at the requested slice. The comparison between the pre-fix and post-fix receipt indexes changes only `ordinary-update`: its evidence-stage observation changes from one `conflicting` `north-initial` item to one `current` item.

| Pair | Run IDs / duration | Verdict and coverage | Primary shadow metrics (evidence / decision / probe) | Evidence-stage temporal metrics (evidence / decision / probe) | Report SHA-256 | Receipt index SHA-256 |
| --- | --- | --- | --- | --- | --- | --- |
| Pre-fix A/B | `run-1795229274` / 184862 ms; `run-1795229472` / 186033 ms | Both: quality failed, budget passed; 10/10 valid, 1 passed, 9 quality failures, 0 invalid; 0 promotion gates | 0.60 / 0.10 / 0.10 | 0.90 / 0.70 / 0.70 | A `f9043c504890c46453ea8318442bb552068e6d5de5449d80ec5e754c4864150a`; B `5a9c114ab87eb6acbe1468dbd4a0d2f1dd73d95cfd3bbaf39208f9512be68612` | A `151f4c9e96c208bf2c5fabc2fd7247b68e64580e81945c4a39eb63e66f8a6f3f`; B `0c47de5f72947e475d49c5ae9e9470013fb9854a786c9d98358f96b1e1cc6af0` |
| Post-fix A/B | `run-1795229870` / 191035 ms; `run-1795230074` / 184615 ms | Both: quality failed, budget passed; 10/10 valid, 1 passed, 9 quality failures, 0 invalid; 0 promotion gates | 0.60 / 0.10 / 0.10 | 0.90 / 0.80 / 0.80 | A `7d45f3730f3ef8b8cab2ea3055630a2f4831281db02f4478ba20ccd1d031cf0a`; B `7863057df990c5fa4adfea4af6f31959ade22b340543593948195be6135e4cad` | A `a15ed8a8f8c2efdca36f8cba7a97d48edb571847d91bc2838620c198cf3dcc2a`; B `db3b530ee6b78ef61918b9dce0c297db398922237b3e4ad494d5ec9470baccb2` |

The two post-fix receipt indexes matched on every normalized case after removing only run-specific receipt paths and hashes. Artifacts and receipts remain available under `/tmp/memory-decisions-temporal-v2-{final,postfix}-{a,b}`. The one-case decision/probe increase is deterministic on this small development corpus; it does not establish general memory accuracy. The then-default `shadow` metrics did not improve, and no paired model-agent outcome was measured. The independent `temporal_*` diagnostic remained at 0.90 evidence, 0.00 decision, and 0.00 probe. The development profile has no promotion gates, so “quality failed” is the expected diagnostic verdict.

Post-change regression controls: claim reconciliation passed 41/41 with precision/recall/F1 all 1.0 (report SHA-256 `c01d5dc8b9f1a6ce16005805635f1250003f57fa86a71d6b44606e37f549479d`); retrieval quality passed 37/37 and all six configured floors remained 1.0 (report SHA-256 `1913a77ff22afe58be5fdfaeda65437823ee0df991d6c5209b5b93ee6a3cee5d`). These controls show no observed regression in their current corpora, not a gain on temporal decisions.

The full `memory_mcp` test run reported 2,048 passed and 3 failed; all three failures were watcher startup errors (`unable to start FSEvent stream`) in `fs_watch_integration`, including with `TMPDIR=/tmp`. The targeted temporal integration suite passed 8/8, the knowledge-revision suite passed 11/11, the eval harness passed 522 tests with 17 ignored, and workspace clippy passed with `-D warnings`. The complete crate test command therefore remains environmentally incomplete.

No agent-usability or product-wide improvement is claimed. A descriptive same-product Codex stdio test-retest now exists, but no pilot-derived power analysis, frozen disjoint held-out cohort, or paired product comparison has been completed. The paired Codex HTTP adapter has passed protocol simulation and an isolated live seed smoke, but no Codex task cohort has run through HTTP. The recommendation to keep the rollout at `shadow` was made at this historical baseline; the product default was later changed on the finite deterministic public-MCP evidence in [the current rollout record](../../evals/paired/claim-rollout-current-master-2026-11-21/README.md). The separate agent-usability gates remain open.

## Disjoint temporal diagnostic cohort (exploratory, not confirmatory)

On 2026-11-21, the candidate was run twice against `memory-decisions-v3`, a separate nine-probe corpus covering retroactive correction, prior-knowledge snapshots, valid-time updates, open and reversed conflicts, unverified correction wording, abstention, unrelated distractors and stable repetition. All nine probes have distinct `family_id` values. Event IDs, source IDs and the explicit `subject:` entities are disjoint from v1 and v2. Each probe had one oracle, every oracle evidence reference was inside its probe history, and all nine exact revision snapshots validated. This is a small, manually authored diagnostic cohort: it is **not** the pilot-powered confirmatory cohort in the plan, and the results must not be used to tune and re-score v3.

Both runs used the same product and evaluator executables, configuration, corpus, schema and profile. The product worktree was dirty at base `HEAD` `1ae1da4a4e3120f2de94753fc1a949caee3aced7`; the binary SHA-256, rather than that base commit alone, identifies the candidate. Provider configuration was embedded local SurrealDB, namespace `main`, embeddings disabled, lexical/episode-fallback retrieval, built-in regex NER, primary claim rollout `shadow`, and server-assigned transaction time. The profile had no quality gates. Thus `QUALITY FAILED` is a diagnostic score, not an invalid execution: both runs used all nine cases and reported zero invalid cases with budget passed.

| Run | Run ID / duration | Coverage | Shadow evidence / decision / probe | Temporal evidence / decision / probe | Evidence-stage temporal evidence / decision / probe | Report SHA-256 | Receipt index SHA-256* |
| --- | --- | --- | --- | --- | --- | --- | --- |
| A | `run-1795235027` / 168621 ms | 9/9 valid, 0 invalid | 0.556 / 0 / 0 | 0.667 / 0 / 0 | 0.667 / 0.111 / 0.111 | `02e5652b07e9a035ada2b53b682871148ac8d1a2b4c77554262d0740badf7436` | `353cc4c847d54195b72e6a6285d54293d5c84e6a526f1978e3c4fd6b652952aa` |
| B | `run-1795235196` / 160710 ms | 9/9 valid, 0 invalid | 0.556 / 0 / 0 | 0.667 / 0 / 0 | 0.667 / 0.111 / 0.111 | `780765683bcc650062a8c266896f668a90ba6c8bd6b19370f401b2718b09f124` | `d0cc793fdfb7e9be3f19dcc48c9a6400afe7b98a575f6f5fd25844de18269a61` |

\* The receipt-index hashes are the evaluator's recorded SHA-256 of serialized index bytes before the final newline. Full files are under `/tmp/memory-decisions-v3-{a,b}/index.json`; report files and sibling receipt directories are `/tmp/memory-decisions-v3-{a,b}.json` and `/tmp/memory-decisions-v3-{a,b}/`.

The evaluator replay comparator passed. It compared case order, source order, roles, disposition, shadow scores, both temporal observations and scores, and reason codes; it ignores only run-specific receipt paths and hashes. Shared input identities were: product binary SHA-256 `dcdb395c8b61d33e5120f436773a9e9468c4c777981b065b584833b59a3158f4`; evaluator binary SHA-256 `060ed9dd153da446986b1d27c42a4de9a9c692aebde0013f4bd7eae7ab90300c`; combined corpus SHA-256 `012dab24e7a9a06a4063f931c6695ace45820eda02e30bd3c738830c5a15db41`; input-schema SHA-256 `7b8c60a7d6bdc5d50a71b04e5d2d3dc514516dd69b46627f5b0481cd41c20e61`; provider-config SHA-256 `3144a7d443da339afa86af571d95cc486ae959fa28c61e2610598194fdbcc483`; profile SHA-256 `87147ef6d2d5452c742018f0677eb21d15d95be3885575056f1ed71a10a83b6f`; and `Cargo.lock` SHA-256 `d5879428097dfb38ae0eda3470964fdd5331e5747f6b2d6d50e7e806f530f5d3`. The event, probe and oracle file hashes are `19ae62adc971bc0ace590a76b706d5a1b03e45bc94715989e9707c8c585f7245`, `809d564d2fb06c39335b6d4ddbfe7bbd9daa4211aa150500e037da6868d2a4e8`, and `4e72653f8402d2112a62664cdb4648d127f9d7ce8dec9c50759d30779fbc1958` respectively.

The result separates temporal visibility from agent-decision readiness. The evidence-stage temporal branch supplied an oracle-compatible evidence set in 6/9 cases, but typed decision success was 1/9 and complete probe success was 1/9. It exposed `current` only for the stable repeat. Other evidence-complete cases returned `unknown` roles and generic `supported` disposition. The other visible misses were the unverified correction, where only the correction surfaced as `conflicting`; an unrelated Remote Drum returned for the Cedar Sluice abstention query; and a Willow Shed distractor returned alongside Willow Gate. These are concrete candidate gaps in conflict projection/role output, abstention and entity discrimination. Because v3 is now observed, use it only as a frozen diagnostic; any tuning requires new development cases and a newly held-out cohort.

Reproduce by using the `memory_decisions` command above with profile `evals/profiles/memory_decisions_temporal_heldout.json` and distinct fresh `MEMORY_DECISIONS_ARTIFACT_ROOT` and `--artifact` paths for each run. Both original reports, immutable receipts and indexes remain in `/tmp` on the machine used for these runs.

Latest repository verification (2026-11-21): `cargo test -p eval-harness` passed 554 tests with 19 ignored, including test-retest outcome summaries and nested MCP error classification. The paired schema v2 labels the provider-revision or same-alias method and preserves the UTC invocation window without mislabeling a requested alias as a backend revision. The public HTTP protocol-conformance target, run with `streamable-http,test-fixtures`, passed 36 tests; separate opt-in seed and exact-checkpoint smoke tests passed against an isolated `memory_mcp_http` binary. The Codex HTTP config and credential-forwarding tests verify that bearer/API credentials stay out of command arguments, and the local HTTP fixture generates its ephemeral bearer from OS entropy. `knowledge_revisions` with `eval-support` passed 13 tests, including the revision wait/transaction observer. Strict workspace clippy passes with `-D warnings`, and `cargo fmt --all --check` passes. The latest full `cargo test -p memory_mcp` run reported 2,052 passed and 1 ignored in its main test binary; three `fs_watch_integration` cases could not start FSEvents in this managed macOS environment, so that crate-wide command remains environmentally incomplete. These checks establish implementation and transport behavior only; they do not change the v3 scores or support a memory-quality or usability claim.

## Paired candidate feasibility after the deterministic baseline

The optional agent comparison has now run once over stdio and twice over
Streamable HTTP on the same eight v3-derived development families. Candidate
task-success estimates were 0, +0.125 and 0; all three 95% family-cluster
intervals include zero or negative values. The candidate has one or two hard
temporal violations in every run. Resource point estimates favor the candidate
in these traces, but they do not clear the task-success or zero-violation
requirements. Detailed per-family receipts, intervals, identities, and the
reproducible no-go decision are in the
[paired feasibility report](TEMPORAL_CANDIDATE_AGENT_EVAL.md). Do not calculate
a confirmatory `N` from this cohort: the candidate fails the feasibility gate,
all eight temporal families have already been observed, and no ordinary
workflow stratum or justified target alternative is registered. Keep
`shadow`; build new deterministic cases and a new candidate before considering
a powered agent comparison.

## Corrected temporal role baseline (v10, 2026-11-23)

The v9 audit found an oracle-scope error: ordinary reads were scored for a
`current` role even though the public contract only guarantees temporal roles
on revision-visible snapshots. The historical v9 cases and their artifacts
were left unchanged. Oracle schema v2 now rejects nonempty `expected_roles`
unless the probe requests separate valid-time and knowledge-revision cutoffs.
The new v10 corpus has eight synthetic families, disjoint from v1-v9. Six
cases exercise explicit snapshot roles or relation outcomes; two are ordinary
retrieval/abstention controls without a role assertion.

The same product binary and corpus were captured twice with `default` and
twice with `shadow`. Run pairs match under `compare-decision-replays` for both
arms. No model, agent, Codex service, LM Studio endpoint, API key or external
network call participates in this deterministic public-MCP evaluation.

| Arm | Runs | Primary decisions | Temporal decisions | Evidence | Temporal evidence | Invalid |
| --- | --- | --- | --- | --- | --- | --- |
| `default` (`product_default` / `evidence`) | `run-1795438241`, `run-1795438643` | 8/8 each | 7/8 each | 8/8 each | 8/8 each | 0 |
| `shadow` | `run-1795438449`, `run-1795438807` | 2/8 each | 1/8 each | 8/8 each | 8/8 each | 0 |

The paired primary comparison is six wins, zero losses, and two ties for
`evidence`; all six wins are the cases that require typed roles or relation
outcomes. Evidence retrieval remains 8/8 in both arms. The one temporal miss
in each arm is the empty partial snapshot for the absent-target case: the
ordinary query correctly abstains, while the historical snapshot correctly
reports that partial coverage cannot prove global absence. This is a safe
unknown/partial boundary, not a reason to infer absence or invent a role.

This supports a narrow deterministic claim: on these eight synthetic
histories, the default `evidence` stage improves measured decision readiness
over `shadow` without reducing evidence success. It does not establish broad
agent utility, generalization beyond the fixtures, or powered statistical
significance. The cohort is now exposed and is a regression set only; any
future confirmation must register new disjoint families before tuning.

Execution identity:

- Base `HEAD`: `1ae1da4a4e3120f2de94753fc1a949caee3aced7`; working tree was dirty.
- Product binary SHA-256: `3f4506f9a512782f1c63da7209772ab9ac342be84b84285ef9339f5abf091586`.
- Combined corpus SHA-256: `19b4e9890df5f5706cf99b97638341c2db1ea54dc86f16e8f4e04696e9caaa0e`.
- Input schema SHA-256: `9475b795bde1360eda3cf672b8c75d99ff1fa6cd0e91b219907a7fc81c71880f`.
- Default / shadow config SHA-256: `17ff6b862de83dce474a4e94ab75db5c32b7965b4b8e8265002c338df04de892` / `d52b1ccd6308b544317bb2b9ca8b10ca5fcfe9f6d3a39c59419db0a5ee42755a`.
- Default / shadow profile SHA-256: `3cb72b4cece52519aa0a7225660c47fcbe3ec2106b84d44026da526a6892a22e` / `997e872043c493827f4e1daf666a80bcf9b21137d01e7f0a705eee5b2be59602`.
- Runtime: `rustc 1.100.0`, macOS/aarch64, debug build, no optional features.

Reports, normalized indexes and per-case raw public-MCP receipts are archived
under [`evals/paired/memory-decisions-v10-baseline-2026-11-23`](../../evals/paired/memory-decisions-v10-baseline-2026-11-23/).
