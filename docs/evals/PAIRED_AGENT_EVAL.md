# Paired agent-memory evaluation

> **2026-11-24 typed-probe update:** independent fixed-query oracles can now
> require returned claim annotations (`evidence_status` and compact
> reconciliation outcome/role pairs) in addition to sufficient source IDs.
> The [one-item synthetic conflict control](../../evals/paired/typed-evidence-probe-2026-11-24/README.md)
> scores `evidence` true and `shadow` false on separate fresh stores and the
> same product SHA. This is a deterministic memory-output result, not agent
> usability evidence or a powered treatment estimate. See
> [independent probe contract](INDEPENDENT_MEMORY_PROBES.md).

> Audit correction (2026-11-21): existing paired direct-memory scores are agent-observed source coverage, not independent fixed-query probes. Untyped forbidden sources now receive `prohibited_evidence`; historical `temporal_mismatch` receipts may include unrelated evidence. See the [current follow-up design](../superpowers/specs/2026-11-24-agent-memory-follow-up-design.md).

Use [the deterministic memory-decision profiles](README.md#deterministic-memory-decisions)
to evaluate memory behavior directly. For downstream task and interaction
metrics, use a fixed agent runner to capture trials and score the observations
locally. The scorer uses fixed rules and accepts the same data contract from
any runner. `run-paired-codex` and `run-paired-chat-completions` are optional
live adapters; the deterministic profiles and offline scorer do not invoke
either one.

The deterministic profiles and offline scorer do not require Codex, LM Studio,
another model provider, an API key, or an agent harness. The checked-in
evaluation tests use scripted inputs and controlled adapters. When using an
external runner, it may be any harness that exports the versioned observation
contract below; use `cargo run` so Cargo selects the host executable on Windows,
Linux, and macOS.

LM Studio is an optional local model provider, not a test prerequisite. The
provider-neutral `run-paired-chat-completions` adapter accepts a local server
at a numeric IPv4/IPv6 loopback address that implements the OpenAI-compatible
Chat Completions tool-call contract.
LM Studio documents that interface at
[`/v1/chat/completions`](https://lmstudio.ai/docs/developer/openai-compat/chat-completions)
and provides the optional `lms` CLI to manage its models and server
([CLI reference](https://lmstudio.ai/docs/cli)). No LM Studio package, CLI,
account, or model is used by the default tests or deterministic evaluation.

The adapter executes the advertised memory operations through the public
stdio MCP interface. It accepts only plain HTTP URLs whose host is the numeric
IPv4 or IPv6 loopback address, disables proxies and redirects, and sends an
optional `MEMORY_EVAL_BEARER_TOKEN` only to that endpoint. The transport check
does not prove where the model server computes: a loopback service may itself
forward requests elsewhere, so configure a genuinely local provider if task
and memory text must remain on this machine. Both paired arms must use the
same endpoint and model identifier. The adapter pins the model identifier,
memory binaries, tool schema, prompt and policy hashes, and evaluator binary
in the plan. The server's model weights are not independently fingerprinted;
record the provider's immutable model/runtime revision in the plan and report
if it exposes one.

The live adapter is an optional model-backed workflow, not a deterministic
memory benchmark. Model responses may vary. Its normalized receipts can be
scored and compared reproducibly after capture; use the provider-independent
fixed-profile tests for deterministic memory-quality acceptance. Contract
tests use a fake loopback HTTP server and fake public-MCP session, so they run
without LM Studio or any model server. The test target has no OS-specific
provider dependency and can be run with Cargo on Windows, Linux, and macOS; the
local smoke below has only been exercised on this macOS laptop.

If a Chat Completions response includes `reasoning_content` alongside a tool
call, the adapter forwards that optional field in the next assistant-history
message for providers that require it. It remains transient request context and
is not copied into receipts, traces, or normalized observations.

An additional ignored compatibility smoke is available for contributors who
already run any compatible local provider. The normal test target skips it:

Run the provider-independent contract tests on any supported host with:

```sh
cargo test --locked -p eval-harness --test chat_completions_runner
```

This starts only the in-process fake HTTP/MCP test fixtures. It needs no model,
LM Studio installation, API key, or provider server. The optional local smoke
below is separate.

```sh
# Optional structured-output/tool-call compatibility check:
# export MEMORY_EVAL_CHAT_COMPLETIONS_RESPONSE_FORMAT=./response-format.json
MEMORY_EVAL_CHAT_COMPLETIONS_ENDPOINT=http://127.0.0.1:1234/v1 \
MEMORY_EVAL_CHAT_COMPLETIONS_MODEL=<exact-model-id> \
cargo test --locked -p eval-harness --test chat_completions_runner \
  local_chat_completions_smoke_executes_a_public_memory_tool -- --ignored --exact
```

```powershell
$env:MEMORY_EVAL_CHAT_COMPLETIONS_ENDPOINT = "http://127.0.0.1:1234/v1"
$env:MEMORY_EVAL_CHAT_COMPLETIONS_MODEL = "<exact-model-id>"
$env:MEMORY_EVAL_CHAT_COMPLETIONS_RESPONSE_FORMAT = ".\response-format.json" # optional
cargo test --locked -p eval-harness --test chat_completions_runner local_chat_completions_smoke_executes_a_public_memory_tool -- --ignored --exact
```

This smoke uses synthetic text and an in-memory fake MCP session. It checks
tool-call compatibility only; it does not capture or score an agent workflow,
does not count as a memory-quality result, and is not run by CI. Set the
endpoint and model to match the contributor's provider. To verify structured
output together with tool calls, set the optional
`MEMORY_EVAL_CHAT_COMPLETIONS_RESPONSE_FORMAT` path to a JSON response-format
file. LM Studio's `lms ps`
and `/v1/models` can show its loaded model identifier, but those commands are
not called by the harness.

To run the optional source-authority workflow, first build the product and
create a new disjoint plan that pins `agent_runner` to
`openai-compatible-chat-completions`, the exact model identifier, prompt and
policy fingerprints, product and evaluator binary hashes, budgets, and task
manifest. Do not substitute this runner/model into a frozen Codex plan. The
following command shape works from POSIX shells and PowerShell; use the
platform's executable path (`memory_mcp.exe` on Windows):

For literal-source source-authority trials, use task manifest schema
`memory-mcp-agent-managed-tasks/v3`. The incumbent receives the exact factual
text in `formation_prompt`; candidate file bytes are imported through the
registered operator setup before read-only recall. `ingest.content` is literal
text and is never a path-reading operation. The v3 fingerprint binds the
literal-input protocol; historical v1/v2 fingerprints keep their original
protocol identities.

```sh
cargo build --locked -p memory_mcp
cargo run --locked -p eval-harness --bin memory-eval -- run-paired-chat-completions \
  --plan ./new-plan.json --tasks ./new-tasks.json \
  --model-endpoint http://127.0.0.1:1234/v1 \
  --incumbent-memory-server "$PWD/target/debug/memory_mcp" \
  --candidate-memory-server "$PWD/target/debug/memory_mcp" \
  --output-directory "$PWD/new-run"
```

Equivalent PowerShell form:

```powershell
cargo build --locked -p memory_mcp
$memoryServer = (Resolve-Path .\target\debug\memory_mcp.exe).Path
$outputDirectory = Join-Path (Get-Location) "new-run"
cargo run --locked -p eval-harness --bin memory-eval -- run-paired-chat-completions --plan .\new-plan.json --tasks .\new-tasks.json --model-endpoint http://127.0.0.1:1234/v1 --incumbent-memory-server $memoryServer --candidate-memory-server $memoryServer --output-directory $outputDirectory
```

`hash-chat-completions-tasks --tasks <path>` emits provider-neutral corpus,
oracle, prompt, policy, simulator and protocol hashes before preregistration.
Add `--model-endpoint http://127.0.0.1:1234/v1` when preparing a local paired
plan; the command then also emits the versioned SHA-256 of the normalized
effective endpoint URL. Copy it to
`shared_identity.provider_fingerprints.chat_completions_endpoint_config`.
If the model needs explicit generation controls, put the exact values in a
JSON file and pass it to both fingerprinting and execution:

```json
{
  "temperature": 1.0,
  "top_p": 0.95,
  "top_k": 20,
  "min_p": 0.0,
  "presence_penalty": 0.0,
  "repeat_penalty": 1.0
}
```

For example, the [Qwen3.8-27B model card](https://huggingface.co/Qwen/Qwen3.8-27B)
recommends those values for its thinking mode. LM Studio's Chat Completions
API documents the compatible `top_k`, `min_p`, `presence_penalty`, and
`repeat_penalty` fields [here](https://lmstudio.ai/docs/developer/openai-compat/chat-completions).

```sh
cargo run --locked -p eval-harness --bin memory-eval -- hash-chat-completions-tasks \
  --tasks ./new-tasks.json --model-endpoint http://127.0.0.1:1234/v1 \
  --sampling-config ./sampling.json
```

Copy `chat_completions_sampling_config_sha256` from the output to
`shared_identity.provider_fingerprints.chat_completions_sampling_config`, then
pass the same file to `run-paired-chat-completions` with `--sampling-config`.
The runner rejects a non-default sampler that is absent from the frozen plan,
or any sampler whose hash differs. Without a file, the legacy configuration is
`temperature: 0` with no overrides. Each provider must support the requested
parameters; `top_k`, `min_p`, and `repeat_penalty` are not part of every
OpenAI-compatible server's contract. The sampler file contains no endpoint or
credential, and its tests use a fake loopback server only.

The runner also supports an optional `--response-format-config` JSON file for
providers that can combine structured output with tool calls. It is omitted by
default to preserve existing plans and provider compatibility. When enabled,
the runner sends the exact format on both tool-call and final-answer turns; the
active format must match the plan fingerprint
`shared_identity.provider_fingerprints.chat_completions_response_format_config`.
The hash command validates and emits this fingerprint without contacting a
model. LM Studio documents structured output and tool use separately; verify
their combination with the exact model and server before preregistering a
capture. A fake endpoint contract test proves request wiring only.

Example `response-format.json` for the source-authority recall answer:

```json
{
  "type": "json_schema",
  "json_schema": {
    "name": "memory_recall_answer",
    "strict": true,
    "schema": {
      "type": "object",
      "properties": {
        "answer": { "type": "string" },
        "citations": { "type": "array", "items": { "type": "string" } },
        "needs_clarification": { "type": "boolean" }
      },
      "required": ["answer", "citations", "needs_clarification"],
      "additionalProperties": false
    }
  }
}
```

Register that exact file with `hash-chat-completions-tasks --response-format-config
./response-format.json`; copy
`chat_completions_response_format_config_sha256` into the plan's provider
fingerprints, then pass the same file to
`run-paired-chat-completions --response-format-config ./response-format.json`.
The POSIX and PowerShell command shapes are otherwise identical; use the
platform's normal path syntax. The config is provider-neutral and optional, and
does not change deterministic scoring or tests.

Some local models need provider-specific request parameters that are not part
of the OpenAI-compatible contract. `--request-parameters-config` accepts a JSON
object of such top-level parameters, sends it unchanged on every turn, and
requires its versioned fingerprint in
`shared_identity.provider_fingerprints.chat_completions_request_parameters_config`.
The hash command validates and emits that fingerprint. The extra object cannot
override model, messages, tools, tool choice, token budgets, sampling, response
format, or streaming fields; the runner continues to own those protocol rules.
This is an optional extension point for any compatible server, not an LM Studio
dependency, and provider-specific options may not work on other servers.

For example, a model template may expose a thinking toggle:

```json
{
  "chat_template_kwargs": { "enable_thinking": false }
}
```

The Underdog Saluki model card recommends this exact parameter with thinking
off and temperature zero for direct tool calls; it reports its own BFCL score,
which is publisher evidence and not a substitute for this project's frozen
qualification. The card also identifies the model as CC BY-NC 4.0, so check
that license before using it outside non-commercial evaluation. [Model card](https://huggingface.co/ConwayResearch/Underdog-Saluki-27B-1.0)
LM Studio documents model-specific custom fields that affect Jinja variables,
but its Chat Completions parameter list does not promise arbitrary
`chat_template_kwargs` support. This evaluation therefore records the parameter
as an exact-provider extension and relies on the synthetic live smoke for that
specific loaded model/server pair; the runner makes no cross-provider support
claim. [LM Studio model.yaml docs](https://lmstudio.ai/docs/app/modelyaml),
[Chat Completions docs](https://lmstudio.ai/docs/developer/openai-compat/chat-completions).

Register it with `hash-chat-completions-tasks --request-parameters-config
./request-parameters.json`; copy
`chat_completions_request_parameters_config_sha256` into the paired plan and
pass the same file to `run-paired-chat-completions --request-parameters-config
./request-parameters.json`. The exact JSON is caller-supplied and must be
validated against the target model server before capture. It is hash-pinned but
never copied into receipts. Deterministic scoring and fake-endpoint tests remain
independent of any provider.

The live runner rejects a missing or mismatched endpoint fingerprint before
opening a trial, and checks the active `memory-eval` binary and both product
hashes against the plan. The same binary path
is valid only when both pinned arms intentionally use the same product; a
product comparison must pass two executable paths whose hashes match the
incumbent and candidate identities. Normal test and scoring commands never
read the model endpoint or `MEMORY_EVAL_BEARER_TOKEN`.

### Current local-model screens

The LFM2.5-1.2B feasibility pilot completed its model calls quickly
(5.1 seconds total), but all task answers failed the frozen oracle (0/4 per
arm); the full four-family process still took 73.9 seconds. A disjoint failure
diagnostic also scored 0/4. The local Qwen3.8-27B v4 screen took 11m33s and
scored 0/4 per arm, with malformed-answer/tool-call failures and one budget
overrun. A disjoint v5 screen using the model-card thinking-mode sampler took
10m18s and scored 1/4 incumbent versus 0/4 candidate; independent probes passed
3/4 per arm and candidate file-import tasks passed 0/2. This also failed its
frozen gate. These were separate, non-matched cohorts, so their wall times are not
a controlled speed comparison. Neither model/configuration passed the
registered task floor; neither result is evidence for or against a memory
change. See the [LFM pilot](../../evals/paired/source-authority-local-lfm25-pilot-2026-11-23/run-01-report.md),
[LFM failure diagnostic](../../evals/paired/source-authority-local-lfm25-diagnostic-2026-11-23/post-capture-audit.md),
and [Qwen screen](../../evals/paired/source-authority-local-chat-completions-v4-qualification/runs/run-02/report.md),
[Qwen corrected-sampler screen](../../evals/paired/source-authority-local-chat-completions-v5-sampler-qualification/runs/run-01/report.md).
Keep LFM opt-in for adapter/tool smoke checks unless a new, disjoint
qualification passes its frozen task and probe floors.

A disjoint Qwen v6 screen used the reasoning-history adapter fix and the v3
literal-input protocol. It completed in 16m53s with task-oracle success 0/4 in
both arms, independent probes 2/4 incumbent and 3/4 candidate, candidate
source-file task success 0/2, and zero hard violations or budget overruns.
Both file imports and extractions succeeded, but the task and probe floors
failed. This is a NO-GO for that exact model/runner configuration, not a memory
effect; do not replay the exposed cohort. See the [Qwen v6 report](../../evals/paired/source-authority-local-chat-completions-v6-runner-fix-qualification/runs/run-01/report.md).

A fresh Flint screen used four new families, the literal-input protocol, and a
Chat Completions adapter that preserves optional `reasoning_content` through
tool-call follow-ups. It completed in 2m24s: task-oracle success was 0/4 in
both arms, independent probes were 1/4 incumbent and 3/4 candidate, and
candidate source-file recall passed 0/2. Both candidate file imports and
extractions succeeded; hard violations and budget overruns were zero. Seven
arm receipts ended in `final_answer_not_json`; the eighth was schema-valid but
incorrect. This is NO-GO for the tested Flint configuration, not a memory
effect or model-wide conclusion. Do not rerun this cohort. See the [Flint run
report](../../evals/paired/source-authority-local-flint-v1-qualification/runs/run-01/report.md).

A fresh Qwen Splash screen then tested the registered JSON Schema format on
both tool and final-answer turns. All eight final answers were schema-valid,
but task-oracle success was 0/4 in each arm; independent probes were 1/4
incumbent versus 3/4 candidate, and candidate source-file task success was 0/2.
Candidate imports and extractions passed 2/2, with no hard violations or
budget overruns. This is NO-GO for that exact model/runner configuration and
does not establish a memory benefit. See the [Qwen Splash run report](../../evals/paired/source-authority-local-qwen-splash-response-format-v1-qualification/runs/run-01/report.md).

The Underdog Saluki v1 screen was technically invalid as a model-quality
capture: all eight requests failed before consuming model tokens or making
memory-tool calls. A controlled request-shape repro showed the local server
rejecting two consecutive system messages and accepting the runner's corrected
single system message. The artifacts remain preserved as a transport audit.
The provider-neutral runner now supports hash-pinned optional provider request
parameters; neither this extension nor its contract tests require LM Studio.
See the [v1 audit](../../evals/paired/source-authority-local-underdog-saluki-v1-qualification/post-capture-audit.md).

A fresh, disjoint Underdog Saluki v2 screen applied the model card's direct
tool-call settings (thinking disabled, temperature zero) and omitted JSON
Schema output after an exact-model smoke showed that schema mode suppressed the
tool call. The frozen 4-family capture completed in 13m28s and passed offline
artifact comparison. Task-oracle success was 0/4 per arm; independent probes
were 2/4 incumbent versus 4/4 candidate; candidate source-file task success was
0/2; four arm trials exceeded their budget; operator imports/extractions
succeeded and hard violations were zero. All eight trials lacked an accepted
final answer. The exact configuration is NO-GO. The +0.5 paired direct-memory
signal (95% family-cluster interval [0, 1]) is exploratory and cannot qualify
agent usability or demonstrate a memory implementation effect. The model card
reports its own tool-use benchmark, but that does not generalize to this task
set; the card is also CC BY-NC 4.0. See the [v2 report](../../evals/paired/source-authority-local-underdog-saluki-v2-qualification/runs/run-01/report.md)
and the [model card](https://huggingface.co/ConwayResearch/Underdog-Saluki-27B-1.0).

## Score an external runner's trials

```sh
cargo run --locked -p eval-harness --bin memory-eval -- export-paired-inputs \
  --plan plan.json --tasks private-tasks.json --output runner-inputs.json

# Run the two arms with your runner and save normalized observations.json.
cargo run --locked -p eval-harness --bin memory-eval -- score-paired-observations \
  --plan plan.json --tasks private-tasks.json \
  --observations observations.json --output-directory ./new-scored-run
```

Equivalent PowerShell commands (the one-line form also works in POSIX shells):

```powershell
cargo run --locked -p eval-harness --bin memory-eval -- export-paired-inputs --plan plan.json --tasks private-tasks.json --output runner-inputs.json
cargo run --locked -p eval-harness --bin memory-eval -- score-paired-observations --plan plan.json --tasks private-tasks.json --observations observations.json --output-directory new-scored-run
```

Create the plan with the actual runner, model, prompts, budgets, products and
transport for this experiment. The checked-in Codex plans bind Codex's setup.
The export checks the plan and emits `memory-mcp-agent-inputs/v1` with
`plan_sha256`, `incumbent_identity`, `candidate_identity`, and `tasks`.
Each task contains its `key`, prompt, ordered seed episodes and optional
`knowledge_before_event_id`. Give the runner this export and keep
`private-tasks.json` with the evaluator.

For stage-sensitive comparisons, pin `claim_rollout_stage` independently in
both product identities in the plan (for example `shadow` for the incumbent
and `evidence` for the candidate). The Codex adapter resolves each arm from
that frozen identity. The runner-neutral independent-probe adapter applies the
same per-arm treatment while keeping each probe's query, principal, history,
snapshot and item budget fixed; the reference probe is registered at the
incumbent stage, and only the pinned rollout-stage field may differ in the
candidate capture. Legacy plans without the field still resolve both arms to
`evidence`; `--claim-rollout-stage` remains available as a common override for
those plans and is rejected if it conflicts with a pinned stage.

Record a JSON object with the following fields. The wire types are
[`RecordedPair`](../../crates/eval-harness/src/paired_agent/observations.rs),
[`AgentObservation`](../../crates/eval-harness/src/paired_agent.rs), and
[`AgentTraceEvent`](../../crates/eval-harness/src/paired.rs).

| Level | Fields and recording rules |
|---|---|
| Bundle | `schema_version: "memory-mcp-agent-observations/v3"`, exported `plan_sha256`, unique capture `run_id`, RFC 3339 UTC `run_started_at` and `run_completed_at`, and `incumbent` and `candidate` arm objects. The scorer continues to read v1, v2 and v3 observations. |
| Arm | `identity` matching the exported identity for that arm, and `trials` with exactly one entry per planned key, including failed and skipped trials |
| Trial | Exported `key`, actual `arm_order` (1 or 2), `known_revision_checkpoint` when the task specifies a boundary, and `episode_id_to_event_id` mapping stored episode IDs to their original seed events. Seed fresh stores and record the exact checkpoint used in both arms. |
| Observation | `status`, `turns_used`, `tool_calls`, `user_clarifications`, `input_tokens`, `output_tokens`, `wall_time_ms`, `trace`, `memory_source_episode_ids`, and optional `final_answer` |
| Trace | Contiguous `sequence` starting at 1, `turn`, snake-case `kind`, and the applicable `call_id`, `tool_name`, `error_class`, `payload_sha256`, or paired `input_tokens` and `output_tokens`. Use `agent_turn_started` at turn start and `agent_turn` for completed-turn usage. Use `memory_read` for the MCP call and match it with a `tool_response` or `tool_error` by call ID and tool name. |

Record the normalized tool name `assemble_context` and collect episode IDs
from its successful responses. Record the exact final output as a string.
The expected output is JSON text such as
`{"answer":"Juniper","citations":["episode-1"]}`. The receipt checks the
`final_answer` trace event's SHA-256 against the exact UTF-8 bytes of that
string. Malformed final output counts toward the task failure rate. Tool,
model and timeout failures use only the closed error classes from the
trace schema. Keep raw prompts, answers and observations local when sharing
scored receipts.

The scorer validates case coverage, arm order, identity, checkpoints,
trace consistency and resource budgets. The closed `error_class` field is
required on `tool_error`, optional on terminal `model_failure` and `timeout`
events, and forbidden on all other trace events. Provider-specific error text
is never recorded. V2 and v3 observations can use the content-free
`invalid_response`, `invalid_final_answer` and `invalid_tool_call` classes.
V3 observations add
`empty_formation_completion`, `final_answer_not_json` and
`final_answer_schema_mismatch`, distinguishing empty write-phase completions,
non-JSON recall answers and JSON answers with the wrong shape. These granular
v3 observation diagnostics require v4-or-newer paired artifacts. The scorer
now emits paired v6 artifacts with content-free `task_oracle_diagnostics`:
whether a final answer was present and schema-valid, whether its answer and
required citations passed, and whether clarification behavior matched the task
oracle. These booleans never contain answer text, citations, source content or
hidden oracle values. V6 adds `accepted_answer_match`: `exact`,
`mentioned_with_extra_text`, or `no_accepted_value_mentioned`. This is a
diagnostic only: task acceptance still requires the existing exact normalized
answer match, so an answer embedding the accepted value in explanatory text
still fails the oracle. The classification stores no answer or oracle text and
does not establish why a value was absent. Paired artifact readers accept v2
through v6; observation
readers accept v1 through v3. V1 observations reject model-response diagnostics;
v2 rejects the granular classes. Older paired v2-v4 receipts remain valid
without the new component fields; v5 requires its component fields for every
attempted trial and rejects v6-only answer classifications; v6 requires the
answer classification for every schema-valid final answer. V5 artifacts remain
readable without migration. No response text is persisted in these diagnostics.
The scorer writes
`incumbent.json` and `candidate.json` into a new directory. The same frozen plan, task manifest
and observations produce byte-identical artifacts, which feed
`summarize-pilot` for distinct development captures or `compare-paired`
for a confirmatory plan. The capture adapter is responsible for correctly
measuring usage and recording the actual runner, product, tool output, time
and revision. Offline validation checks the recording's internal consistency.

### Exploratory product comparison

Use `stage: "exploratory_comparison"` for a paired diagnostic of different
product builds when no pilot-derived power report or promotion criteria exist.
This stage pins the same agent, prompts, task manifest, budgets and hardware in
both arms, and it still validates product hashes, tool-schema migrations,
receipts and family-level confidence intervals. It cannot carry pilot/power
hashes or promotion criteria. After scoring observations from any compatible
runner, `compare-paired-diagnostic` writes the raw paired comparison and exits
successfully when the artifacts are valid. It deliberately produces no
promotion decision. Do not interpret its intervals as confirmatory evidence.

The deterministic `memory_decisions` profiles are the primary path when the
question is memory behavior itself: they run scripted public MCP inputs and
require no agent, model account, Codex installation or external harness. A
fresh v9 default replay on 2026-11-23 also passed with `PATH=/usr/bin:/bin` and
`CODEX_API_KEY` unset, matching both archived default captures; see the
[replay artifacts](../../evals/paired/extract-evidence-default-2026-11-23/current-audit-repeat/).
Use the paired agent path only for downstream workflow outcomes such as task
success, repair turns, interactions and token/time cost. Its input export and
offline scorer are runner-neutral; the live Codex adapter is optional.

## Optional live Codex adapter

**Status (2026-11-21):** The deterministic comparator, receipt validation,
family-cluster bootstrap, optional promotion gate, and runner-neutral
observation scorer are implemented in `crates/eval-harness/src/paired.rs`,
`crates/eval-harness/src/paired_agent.rs` and
`crates/eval-harness/src/paired_agent/observations.rs`. The scorer consumes
normalized trials from any compatible runner; the direct `memory_decisions`
profiles require no agent or external harness. Codex's stdio and HTTP capture
adapters are optional implementations in
`crates/eval-harness/src/paired_agent/codex.rs` and
`crates/eval-harness/src/paired_http.rs`.

One exploratory candidate comparison completed over stdio, followed by two
valid Streamable HTTP comparisons on the same eight development families. The
first HTTP attempt is retained as invalid transport evidence: Codex sent a
port-qualified Host header but the local fixture allowlist contained only bare
hostnames, so all trials failed before a model turn. The runner now binds an
available loopback port, allows the exact authority, and sends that same Host
header; production host validation was not relaxed. The deterministic HTTP
simulator, live seed/checkpoint smoke, and both fixed-agent HTTP runs pass. All
paired outputs and limitations are in the
[temporal feasibility report](TEMPORAL_CANDIDATE_AGENT_EVAL.md).

The candidate's task-success intervals include zero or negative effects in
all three valid comparisons, and every run has at least one candidate temporal
violation. Lower tool, token and wall-time costs are descriptive on already
observed development families; they do not prove that agents found memory more
useful. The confirmatory sample size remains unset because the candidate fails
the hard temporal gate and the cohort lacks ordinary-workflow cases and a
justified target alternative. Do not claim memory-quality improvement, agent
usability improvement, or promotion. No separate `CODEX_API_KEY` is required
when the optional adapter uses an authenticated local home through
`--codex-home`.

The runner also supports a separate `development_pilot` plan stage. It requires
the same incumbent product identity in both arms and does not accept future
pilot-report or power-analysis hashes or promotion margins. Each invocation
stamps both artifacts with one run ID; `summarize-pilot` requires two distinct
invocation IDs, verifies the same frozen plan and family coverage, and reports
the two run-level and pooled family deltas plus sample variance across
independent family means. Report schema v3 also records per-family baseline
outcome levels and second-minus-first test-retest deltas separately from the
same-product paired null deltas. These quantities describe the observed pilot;
they do not estimate a candidate treatment effect or produce a promotion
decision. This removes the ordering cycle in which pilot execution would
otherwise require hashes and margins that only exist after pilot analysis.

`summarize-pilot` also accepts an `exploratory_comparison` plan for two repeated
arms that intentionally use different product identities (for example,
`shadow` and `evidence`). It emits
`memory-mcp-agent-exploratory-pilot-report/v1`, with per-family outcome and
retest summaries kept separately for incumbent and candidate. It does not
reuse the development v3 `same_product_outcomes` field, which would be
misleading for different products, and it never emits a promotion decision.
Confirmatory plans are rejected by `summarize-pilot`; use `compare-paired`
with the preregistered pilot and power-analysis references for those plans.
The offline summary is deterministic for identical plan and run artifacts.

For an eligible exploratory report, `plan-paired-power` provides a
provider-independent exact power calculation for one binary task outcome per
independent family. It validates both captures, counts a family as discordant
if either retest differs, applies the one-sided Clopper-Pearson upper bound to
the discordance rate, and sums exact McNemar power through the requested sample
cap. Repeated captures do not increase the independent-family count. The
planner reports sample size and assumptions only; it never emits a promotion
decision. A failed frozen pilot gate still blocks confirmation regardless of
the computed power.

```sh
cargo run -p eval-harness --locked --bin memory-eval -- plan-paired-power \
  --pilot-report evals/paired/typed-conflict-agent-pilot-2026-11-24/runs/pilot-summary.json \
  --stratum-id conflict_decisions \
  --target-risk-difference 0.25 \
  --minimum-point-effect 0.10 \
  --one-sided-alpha 0.025 \
  --target-power 0.90 \
  --confidence-level 0.95 \
  --maximum-sample-size 200 \
  --output evals/paired/typed-conflict-agent-pilot-2026-11-24/runs/power-analysis.json
```

The typed-conflict pilot completed two captures but failed its preregistered
ordinary-control floor: the `evidence` arm scored 0/4 ordinary agent tasks in
both captures. Its power report is therefore planning-only and does not
authorize confirmation. See the [pilot record](../../evals/paired/typed-conflict-agent-pilot-2026-11-24/README.md)
for full results and interpretation.

A Codex invocation adapter and unit-tested JSONL parser exist in
`crates/eval-harness/src/codex_agent.rs`. The default adapter launches an
ephemeral, read-only Codex process with shell, web search, apps, plugins and
Codex memories disabled; it configures one required MCP server with only
`memory_decisions/assemble_context` enabled. Agent-managed development pilots
are an explicit opt-in profile: the internal formation phase exposes only
`ingest` and `extract`, then a fresh recall process exposes only
`assemble_context`. `invalidate` and general reads are unavailable during
formation. A separate maintenance profile exposes only
`assemble_context`, `ingest`, `extract` and `invalidate` for one correction,
then uses fresh read-only recall and an independent evaluator probe. This
profile is also opt-in. The server can use stdio or a loopback Streamable HTTP
MCP endpoint.
HTTP trials receive a fresh durable SurrealKV store and short-lived bearer
token; the token is passed to Codex through an environment-variable reference,
not a command argument or artifact. Each trial uses an empty task workspace
separate from its seeded memory store, requires structured output with
citations and per-turn usage, and retains payload hashes instead of raw tool
content in trace events. While streaming bounded JSONL, it enforces wall time,
turn count, unique MCP-call count, aggregate output tokens, and a protective
JSONL-byte ceiling. A wall-time timeout returns a partial attempt trace,
including a model turn or memory call that started without completing. The
receipt validator accepts that failed attempt without inventing token usage or
dropping it from the denominator. Fake-executable integration tests cover each
enforced limit; no model call is made by the test suite.

The harness pins the base instructions with the tracked
[`memory-recall-system-v1.txt`](../../evals/paired/prompts/memory-recall-system-v1.txt)
file via Codex's `model_instructions_file` override, and pins the current
appended task rules with
[`memory-recall-task-rules-v2.txt`](../../evals/paired/prompts/memory-recall-task-rules-v2.txt).
V2 asks for a concise direct value in `answer`, without explanatory wording;
this aligns the prompt with the deterministic exact-value oracle and does not
relax acceptance. The previous
[`memory-recall-task-rules-v1.txt`](../../evals/paired/prompts/memory-recall-task-rules-v1.txt)
remains frozen for historical run identity. New runs must use a plan bound to
the v2 prompt fingerprint; a v1 plan is rejected before model execution. The
plan's system/task prompt SHA-256 values must match those embedded files;
the case-specific prompts and seeded histories are separately bound by the
task-manifest corpus hash. The override is documented in the official
[Codex sample configuration](https://developers.openai.com/codex/config-file/config-sample).

The plan-bound runner invokes this adapter for each arm/trial and scores its
metadata-only observation against the hidden task oracle in the evaluator
process. It publishes `incumbent.json`, `candidate.json`, and append-only
per-arm JSONL receipt journals only after the manifest passes validation. The
HTTP simulator covers the stateless request headers, tool catalog, bearer
authentication, JSON responses and SSE responses. An ignored live smoke test
can run after building the test-fixture HTTP binary; it starts an isolated
server, waits for readiness, seeds an episode and reads its exact revision via
the public API. A separate opt-in stdio smoke exercises the same boundary path.
These checks verify transport, seeding and checkpoint behavior, not agent
quality.

## Agent-managed formation feasibility pilot

The versioned pilot and case-level artifacts live in
[`evals/paired/agent-managed-memory-v1/`](../../evals/paired/agent-managed-memory-v1/README.md).
V4 is preserved as a valid diagnostic baseline. Across two repeats, all 32
formation phases exceeded the 65,536-token input budget; the broad writer
profile made 40 MCP calls per arm/run, including duplicate `ingest` and
`extract` calls, and no returned write IDs were attributable to a task. All 32
independent probes therefore failed source support. V4 measured the adapter's
old formation protocol, not product memory quality.

V5 changed only the optional runner protocol/prompts and uses eight disjoint
synthetic fact families. Its allow-list is `ingest` + `extract` for exactly one
fact, followed by fresh-session read-only recall and a direct evaluator probe.
Both full repeats completed 8/8 tasks in each arm: 32/32 formation phases
completed, 32/32 ingests and extracts succeeded once, the 32 returned episode
IDs were mapped, and 32/32 independent probes were supported. There were zero
unsupported/prohibited sources or hard violations. Mean formation input was
38,770 tokens per trial (maximum 38,827) against the same 65,536 cap; mean
formation elapsed time was 19.3 seconds per trial. The two-run deterministic
pilot summary is byte-identical on replay.

This is a feasibility and runner-cost result only. Both arms use the same
product binary, so it does not estimate a memory treatment effect. Eight
synthetic families, two invocations and one fact per task do not establish
maintenance cost, real workflow non-inferiority, agent productivity or
population reliability. See the preregistration amendment and exact per-run
comparisons in the linked archive; do not pool v4 and v5 or use these reports
as a promotion gate.

## Source-authority migration feasibility

The separate source-authority comparison is preregistered under
[`source-authority-agent-workflow-v2`](../../evals/paired/source-authority-agent-workflow-v2/README.md).
It compares the clean-HEAD relative-file ingest workflow with current
operator-owned file import followed by read-only recall, plus eight ordinary
conversation controls. The frozen cohort has 16 synthetic families and two
repeated invocations. V1 is invalid: its receipts have zero model turns and
zero tokens, and fixture staging conflicted with the empty Codex workspace
contract. The evaluator now gives only the incumbent stdio MCP child an
evaluator-owned source-root cwd; both Codex workspaces remain empty, and the
candidate imports from that root before agent execution. A public runner test
checks both paths and an independent source probe.

The v2 live invocation used the Codex adapter and was rejected by automatic
approval review before process creation because task, memory and fixture text
could be transmitted to the external Codex service. No data was sent and no
model-backed outcome is available for that frozen plan. The user's local-run
authorization does not authorize sending this corpus to an external service.
A separate synthetic LM Studio/Codex smoke used an empty store, but Codex did
not expose the configured memory MCP tool to the model. It produced no paired
trial and no usability observation.

The provider-neutral loopback Chat Completions adapter now provides a local
route for a future paired workflow. Its standalone synthetic smoke did call a
fake public MCP tool, but did not ingest a source fixture or capture a paired
trial. A local comparison still needs a new disjoint preregistration pinned to
the actual runner, model identifier, budgets, products and evaluator. Do not
modify or reuse the frozen Codex v2 plan. The loopback restriction prevents the
adapter from calling a remote endpoint directly, but cannot detect a local
model server that forwards requests elsewhere; verify the provider's routing
before using any non-synthetic data. Until a valid paired capture is scored,
source-authority workflow usability and non-inferiority remain unmeasured.

## Agent-managed maintenance feasibility pilot

The separately preregistered one-correction maintenance pilot and its exact
artifacts are in
[`agent-managed-maintenance-v1/`](../../evals/paired/agent-managed-maintenance-v1/README.md).
Both repeats use the same product binary in both arms, so differences between
the arm labels are not product treatment effects.

Across the two repeats, all 32 independent fixed-query probes found the
registered correction source, with zero unsupported or prohibited sources
and zero hard violations. Every trial made exactly one successful `ingest`,
`extract`, and `invalidate` call. The maintenance workflow still fails the
preregistered feasibility gate: all 32 formation phases were reported as
`budget_exceeded` against the 65,536-token per-turn context limit. Aggregate
input averaged 101,783 tokens per phase and reached 103,504; the maximum phase
time was 64.2 seconds against the 120-second limit.

Task success varied across identical-product arms: repeat 1 was 6/8 in both;
repeat 2 was 3/8 for incumbent and 8/8 for candidate. This unexplained
agent/run variation under an identical product is not evidence of a memory
gain. The small synthetic cohort does not
establish general maintenance usability or productivity. The generic
`paired_agent_comparison.budget_exceeded_trial_count` counts fresh-recall
receipts; formation-phase statuses live in each arm's
`formation_status_counts`. Read the phase-specific fields separately.

The protocol, inputs and evaluator/product/Codex fingerprints were frozen
before the first run. See the archive README for exact counts, replay commands,
hashes and limitations. Do not modify the frozen preregistration to reflect
post-capture interpretation.

### Lower-response-cap maintenance v2

V2 preregistered a 2,048-token cap for each `assemble_context` response and
used eight history families disjoint from v1. The two valid repeats again
failed the feasibility gate: all 32 maintenance formation phases were
`budget_exceeded` against the 65,536-token input/context limit (mean 100,835,
maximum 103,585 tokens per phase). All 32 trials made one successful ingest,
extract and invalidate; all 32 independent probes supported the correction,
with zero unsupported/prohibited sources or hard violations, and all 32
correction episode IDs were identified. Task success was 11/16 incumbent and
12/16 candidate (23/32 total, below the preregistered 32/32 gate) across the
repeats. Both arms used the same product, so this task-score difference is not
a product effect.

The result closes only this frozen v2 measurement, not maintenance feasibility
or agent benefit. V1 and v2 used different families, so their token summaries
do not isolate the response-cap effect. A redesigned workflow needs a new
preregistration before another agent run. The valid runs, invalid host/setup
preflights, frozen inputs, checksums and byte-identical deterministic replay
commands are documented in the [v2 archive](../../evals/paired/agent-managed-maintenance-v2/README.md).

The `run-paired-codex` adapter accepts the explicit profile
`--memory-tool-profile agent_maintenance_v2`; the default remains
`recall_only`. This live adapter is optional. The runner-neutral task and
scoring contracts remain usable by other runners or offline without Codex.

### Current v6 maintenance feasibility — 2026-11-23

The latest disjoint maintenance workflow completed two direct-host captures.
Candidate formation remained within the registered input budget in 16/16
phases, exact correction writes and independent probes passed, and each
candidate capture used 24 tool calls versus 32 for the incumbent. However,
candidate task success was 8/8 in run-01 and 7/8 in run-02 against a frozen
8/8-per-run gate. The same eight histories were repeated, so they are not 16
independent cases; the paired intervals cross zero. This is a workflow
feasibility **NO-GO**, not evidence of memory-quality gain or agent benefit.

Separately, `extract.include_fact_evidence` is now the product default with a
4 KiB all-or-none serialized cap and explicit compact opt-out, accepted under
bounded response-contract and deterministic no-regression checks. Those
product checks do not override the v6 agent-workflow NO-GO. See the
[v6 report](../../evals/paired/agent-managed-maintenance-v6/README.md) and
[response-default report](../../evals/independent-probes/2026-11-23-extract-evidence-default/README.md).

## Exact temporal checkpoints

For a task that asks what the memory contained before a correction or other
history event, set `knowledge_before_event_id` to that event's `event_id` and
include exactly one `{{KNOWN_REVISION}}` marker in the task prompt. The task
manifest's episode order defines the history prefix: the checkpoint is captured
after all earlier episodes have been ingested and extracted, immediately before
the named event is seeded. The candidate binary resolves the marker through a
public `assemble_context` response before either arm runs. Both arms receive the
same resolved prompt. The candidate trial then seeds a fresh store and must
reproduce the prepared revision or the run fails closed. The exact token is
recorded in both trial receipts; the corpus hash binds the prompt template and
boundary event ID. A task without a temporal checkpoint must not contain the
marker. An incumbent whose tool schema cannot accept `known_revision` still
receives the same prompt; any resulting tool failure is part of the paired
outcome.

The runner supports this contract over stdio and Streamable HTTP. Local live
checkpoint smokes do not replace the fixed-agent development pilot, the same-
case HTTP agent cohort or the paired product comparison.

### Agent-managed phase sidecars

New agent-managed stage supplements use
`memory-mcp-agent-managed-run/v5`. A failed formation phase may include an
optional `terminal_error_class` with a normalized enum value from the final
failure trace event, an optional `terminal_http_status_code` when the provider
returned a non-success HTTP response, and an optional closed
`terminal_transport_stage` (`request_send` or `response_body_read`) for a
transport failure before a response or while reading a successful response.
The HTTP status is numeric only; response headers and bodies are discarded.
These fields are content-free: do not add provider response text, prompts, tool
arguments or memory contents to stage receipts. The shared observation wire
format does not serialize the runner-internal status/stage fields. Readers
accept legacy v1 (no operator imports), v2 (operator imports, no terminal error
classes), and v3 (terminal error classes allowed); v1-v3 cannot contain HTTP
status or transport-stage fields. V4 allows the numeric HTTP status but not the
stage. A non-success status must accompany a failed `transport` formation phase;
a transport stage must accompany `transport`/`ModelFailure` or
`timeout`/`TimedOut`, and cannot coexist with an HTTP status. These diagnostics
localize provider and transport failures without retaining their content; they
do not change quality scoring or establish memory or agent benefit.

## Development incumbent pilot

Use a `memory-mcp-agent-plan/v2` plan with `stage: "development_pilot"`, a
development task manifest with at least two independent history families, and
the same incumbent product identity for both arms. Omit
`pilot_report_sha256`, `power_analysis_sha256` and `promotion_criteria`; the
validator rejects these fields in pilot plans. The current ten-case North Depot
development probes are one correlated family and cannot estimate family-level
variance by themselves. A pilot plan must use independently authored
development histories. Keep this corpus separate from the later confirmatory
cohort.

The checked-in exploratory pilot input is
[`evals/paired/temporal-pilot/`](../../evals/paired/temporal-pilot/). Its eight
families are derived from the frozen v3 diagnostic and are therefore
development-only; exclude all v3 source histories and query variants from any
confirmatory cohort. The Amber pre-correction checkpoint case is deliberately
omitted because the frozen incumbent predates `known_revision` and cannot
prepare or score that exact snapshot. Checkpoint cases belong only in a paired
product run where both product interfaces are preregistered and their behavior
is comparable. The sidecar identity files state the effective host, reader,
product, retry and simulator settings; their exact file hashes are pinned in
`plan.json`. The runner validates the plan and manifest before checking for
API-key or local Codex-home authentication, so plan validation does not require
a model call.

Run the same frozen plan and task manifest twice, each time to a new output
directory. To use the existing local Codex login, pass its Codex home explicitly
with `--codex-home "$HOME/.codex"`; this works with ChatGPT login and does not
require `CODEX_API_KEY`. Codex CLI ignores the user config for these runs, while
the trial workspace, task `HOME`, and memory database remain per-trial temporary
directories. The user's authentication store is read from the selected Codex
home and may be refreshed by Codex during use. Check it with
`CODEX_HOME="$HOME/.codex" codex login status` before running. This behavior is
documented in the official [Codex authentication guide](https://developers.openai.com/codex/auth).
The runner pins `approval_policy="on-request"` and
`approvals_reviewer="auto_review"` for the child process because it also uses
`--ignore-user-config`. This routes a requested MCP approval through Codex's
risk-based automatic review without changing the user's persistent config. The
child remains in `read-only` sandbox mode and can call only
`memory_decisions/assemble_context`; shell, web search, apps, plugins and Codex
memories stay disabled. Codex documents the reviewer's approval behavior in its
[automatic review guide](https://developers.openai.com/api/docs/guides/agents-api/sessions/approvals).

Pass the incumbent binary for both product-server options:

```sh
PILOT_BASELINE="$PWD/target/evals/paired/temporal-pilot/memory_mcp-baseline"

cargo run -p eval-harness --locked --bin memory-eval -- run-paired-codex \
  --plan evals/paired/temporal-pilot/plan-context-65536.json \
  --tasks evals/paired/temporal-pilot/tasks.json \
  --codex-binary /opt/homebrew/bin/codex \
  --codex-home "$HOME/.codex" \
  --incumbent-memory-server "$PILOT_BASELINE" \
  --candidate-memory-server "$PILOT_BASELINE" \
  --output-directory /absolute/path/to/new-pilot-run-1

cargo run -p eval-harness --locked --bin memory-eval -- run-paired-codex \
  --plan evals/paired/temporal-pilot/plan-context-65536.json \
  --tasks evals/paired/temporal-pilot/tasks.json \
  --codex-binary /opt/homebrew/bin/codex \
  --codex-home "$HOME/.codex" \
  --incumbent-memory-server "$PILOT_BASELINE" \
  --candidate-memory-server "$PILOT_BASELINE" \
  --output-directory /absolute/path/to/new-pilot-run-2

cargo run -p eval-harness --locked --bin memory-eval -- summarize-pilot \
  --plan evals/paired/temporal-pilot/plan-context-65536.json \
  --first-run-directory /absolute/path/to/new-pilot-run-1 \
  --second-run-directory /absolute/path/to/new-pilot-run-2 \
  --output /absolute/path/to/new-pilot-report.json
```

The two arms in each pilot invocation are repeated incumbent observations, not
a product comparison. The completed pair used the same frozen binary in both
arms and eight development families. Run A recorded task success of 3/8 and
4/8, with direct-memory success of 4/8 in both arms; run B recorded task
success of 4/8 and direct-memory success of 5/8 in both arms. In run B,
`invalid_arguments` occurred in three distinct trials per arm; preserve these
as agent-visible outcomes. The report computes equal-weight family summaries:
task-success means 0.4375 and 0.5000 across the two runs, with test-retest
sample variance 0.2455357; direct-memory means 0.5000 and 0.6250, with variance
0.0535714. See the [pilot report](../../evals/paired/temporal-pilot/results/local-chatgpt-pilot-report-context-65536.json)
and [baseline analysis](MEMORY_DECISION_BASELINE.md); receipt journals contain
only metadata and payload digests.

The local invocations used the existing ChatGPT login and needed no separate
API key; Codex supports ChatGPT sign-in and API-key sign-in as distinct local
authentication methods ([authentication guide](https://developers.openai.com/codex/auth)).
`auto_review` may approve or deny a request based on its risk and the user's
instructions; it is not blanket approval ([review behavior](https://developers.openai.com/api/docs/guides/agents-api/sessions/approvals)).
The first three approval-denied diagnostics and the approval-enabled run that
exceeded the old 32,768-token cap at 36,640 tokens are excluded. The valid pair
uses [plan-context-65536.json](../../evals/paired/temporal-pilot/plan-context-65536.json).
Eight development families do not establish power or a candidate effect. A
joint-rule power analysis still requires a preregistered smallest worthwhile
effect and target alternative, and the disjoint confirmatory cohort remains
unrun. Do not use `compare-paired` or promotion wording on a pilot plan.

To execute a preregistered Codex stdio pair, prepare a plan and a matching task
manifest as described below, then run:

```sh
cargo run -p eval-harness --locked --bin memory-eval -- run-paired-codex \
  --plan evals/paired/temporal-v1/plan.json \
  --tasks evals/paired/temporal-v1/tasks.json \
  --codex-binary /absolute/path/to/codex \
  --incumbent-memory-server /absolute/path/to/incumbent-memory_mcp \
  --candidate-memory-server /absolute/path/to/candidate-memory_mcp \
  --output-directory /absolute/path/to/new-run-directory
```

The plan's shared `transport` identity must be `stdio`. For
`streamable_http`, build each product binary with the fixture-only bootstrap
enabled, pin `transport` as `streamable_http` in the preregistered shared
identity, and pass the corresponding binaries through the same server options:

```sh
cargo build -p memory_mcp --features streamable-http,test-fixtures --bin memory_mcp_http
cargo run -p eval-harness --locked --bin memory-eval -- run-paired-codex \
  --plan evals/paired/temporal-v1-http/plan.json \
  --tasks evals/paired/temporal-v1-http/tasks.json \
  --codex-binary /absolute/path/to/codex \
  --incumbent-memory-server /absolute/path/to/incumbent-memory_mcp_http \
  --candidate-memory-server /absolute/path/to/candidate-memory_mcp_http \
  --output-directory /absolute/path/to/new-run-directory
```

When `--codex-home` is omitted, the paired runner uses a temporary `CODEX_HOME`
and requires `CODEX_API_KEY`; it passes the key only to the Codex child and
does not put it in arguments, logs or artifacts. `--codex-home` and
`CODEX_API_KEY` are mutually exclusive. The HTTP server still receives a
cleared process environment and uses an OS-entropy-generated ephemeral bearer
token through the separate `MEMORY_MCP_EVAL_HTTP_BEARER_TOKEN` variable
referenced by Codex's MCP config. The local HTTP fixture currently requires a
Unix host so it can read cryptographic entropy from the operating system.
The local fixture bootstrap is gated by `test-fixtures`; this command is for
isolated local evaluations and is not a production HTTP deployment. Codex's
documented config supports the `url` and
`bearer_token_env_var` fields ([official MCP configuration](https://developers.openai.com/codex/extend/mcp)); non-interactive execution can receive [`CODEX_API_KEY`](https://developers.openai.com/codex/environment-variables).

The output directory must be new and empty; the runner refuses to overwrite
prior evidence. `--temp-root` can place disposable trial stores on a selected
filesystem, and `--claim-rollout-stage` pins the same supported rollout stage
for both arms. For a confirmatory product run, use `compare-paired` on the two
published run artifacts. A confirmatory result is not eligible for promotion
until the model identity, pilot-derived margins and power plan, held-out
histories, and replayability gates below are satisfied.

## Required inputs

Prepare three JSON files before running either arm:

1. A preregistered `memory-mcp-agent-plan/v2` plan with the full trial manifest,
   independent history-family IDs, a counterbalanced incumbent-first assignment
   for each paired trial, incumbent/candidate product identities, shared
   agent/provider/prompt/transport/simulator identities, budgets, bootstrap
   seed and draws, and all promotion margins.
2. An incumbent run artifact with schema `memory-mcp-agent-paired/v2`, `/v3`,
   `/v4`, `/v5` or `/v6`; v2-v5 remain readable as historical artifacts.
3. A matching candidate run artifact.

The confirmatory plan pins hashes for the pilot report and power analysis. The
comparator validates their SHA-256 shape and commits the complete plan hash into
the comparison; it does not retrieve or authenticate those external files.
Keep the referenced reports and approved plan alongside the run artifacts. A
hash is an identity check, not proof that a pilot or power analysis was actually
performed.
The paired runner keeps the task oracle outside the agent process and derives
`direct_memory_success` and `task_oracle_passed` from the completed attempt and
the hidden evaluator inputs. The comparator validates the resulting artifacts
but treats them as trusted evaluator outputs; it does not cryptographically
attest that a runner produced them. A hand-authored JSON artifact can therefore
still produce a syntactically valid report and must not be treated as empirical
evidence. Preserve the exact runner executable, source revision, plan, task
manifest, oracle and receipts with any accepted experiment.

Run the comparator after both artifacts are complete:

```sh
cargo run -p eval-harness --locked --bin memory-eval -- compare-paired \
  --plan evals/paired/temporal-v1/plan.json \
  --incumbent evals/paired/temporal-v1/incumbent.json \
  --candidate evals/paired/temporal-v1/candidate.json \
  --output evals/paired/temporal-v1/comparison.json
```

Exit codes are `0` for `PROMOTE`, `1` for `REJECT`, `3` for
`INSUFFICIENT EVIDENCE`, and `2` for malformed inputs or failed report
validation. The report is written for all three statistical decisions. Invalid
or missing trial receipts fail closed and do not become passes.

## Scoring contract

- The direct-memory oracle and downstream task oracle are separate outcomes.
  A direct evidence-set pass does not imply a correct agent task.
- Every manifest trial remains in the denominator. Timeouts, model/tool
  failures and budget exits fail task success. An unsuccessful task is charged
  the full preregistered turn budget for the repair-turn metric.
- Repetitions are averaged within a case, cases within a history family, and
  independent families receive equal weight. The deterministic percentile
  bootstrap resamples families with replacement using the preregistered seed.
  It never treats repeated attempts on one history as independent samples.
- The trial manifest preregisters which arm runs first for each pair and
  counterbalances that assignment inside every stratum. Receipts that disagree
  with the schedule are invalid. The report recomputes arm-order counts from the
  frozen manifest.
- Receipts reconcile tool calls, errors, clarifications, model-turn token
  totals, per-turn context limits, terminal events and completion status against
  a metadata-only trace. Payloads are represented by SHA-256 digests; raw
  prompts, responses, memory content and oracle text do not belong in these
  artifacts. Tool, model and timeout errors use only the closed classes
  `invalid_arguments`, `not_found`, `unauthorized`, `timeout`, `rate_limited`,
  `internal`, `transport`, `retryable` and `unknown`; free-form provider errors
  are rejected.
- The comparison records logical SHA-256 identities for both run artifacts,
  the plan and its case manifest. To independently reproduce a report, rerun
  the CLI with those exact run artifacts and the exact plan, then compare the
  output and hashes.

Reported deltas are candidate-minus-incumbent for direct memory success and
task success; resource gains are incumbent-minus-candidate, so positive turn,
tool, clarification, token, wall-time and repair-turn values mean less candidate
cost. Failed tasks are charged the turn budget for both turn and repair-turn
cost. The current gate requires preregistered positive lower-bound gains for
direct evidence, task success and repair turns; task and resource
non-inferiority in every registered ordinary-workflow stratum; resource
non-inferiority overall; zero hard violations; and zero budget exits. A resource
regression is permitted only within its preregistered margin, and an interval
that crosses that margin is insufficient evidence. Margins and sample size must
come from a separate pilot and power analysis before a product comparison. The
power analysis must simulate the joint promotion rule, including the
ordinary-workflow resource guards; marginal power for one endpoint alone does
not size the conjunction. Treat the registered history family as the
independent sampling unit: repetitions and cases sharing one history do not
increase `N`. Analyze within-family candidate-minus-incumbent outcomes rather
than independent-arm rates. This follows evaluation-method guidance to use
paired question differences, account for related question clusters, and plan
around a stated detectable effect ([Miller, *Adding Error Bars to Evals*](https://arxiv.org/abs/2411.00640);
[Kotawala et al., *Resolution Diagnostics for Paired LLM Evaluation*, arXiv
preprint](https://arxiv.org/abs/2605.30315)). The latter also emphasizes
empirical calibration of paired sample-size recommendations; it is not
evidence about this product's treatment effect.

The power plan must specify a target alternative strictly above each decision
margin: sizing a test for an effect equal to the lower-bound threshold cannot
give high power to pass that threshold. Simulate the full joint decision using
the same family-level resampling method and every registered primary and
ordinary-stratum guard. Report the probability that *all* promotion checks
pass, not the largest marginal endpoint power. Record the variance source,
independent-family count, target alternative, method, Monte Carlo uncertainty,
calibration checks and limitations in the hashed power report. Report
effectiveness and resource use separately; source coverage alone does not
establish downstream usefulness ([Tan et al., *MemBench*, Findings of ACL
2025](https://aclanthology.org/2025.findings-acl.989/)).

## Repeated-task reliability (pass^k)

Summarize reliability for an already-scored paired run without launching an
agent or contacting a model provider:

```sh
cargo run --locked -p eval-harness --bin memory-eval -- \\
  summarize-paired-reliability \\
  --plan plan.json \\
  --incumbent incumbent.json \\
  --candidate candidate.json \\
  --output reliability.json
```

The preregistered manifest must contain the same contiguous repetitions
`1..=k` for every case. A case passes `pass^k` only when all its repetitions
pass. The report keeps agent-observed direct-memory coverage separate from
oracle-scored task success; task success requires completion, a passing task
oracle, and no hard violations. It reports overall and per-stratum case rates,
equal-weight history-family rates, and a deterministic family-clustered
bootstrap interval when at least two families are present. Validate the JSON
with the versioned [`PairedReliabilityReport` schema](../../crates/eval-harness/src/paired_reliability.rs)
before comparing it with another run. The output path must not already exist;
the CLI creates reports without overwriting prior evidence.

This is a descriptive reliability summary, not a promotion decision. Repeated
attempts on one history remain one statistical family; they do not increase
the independent-family count or replace a powered, held-out confirmation. The
legacy `direct_memory_success` field remains agent-observed coverage. Use the
separate registered fixed-query probe outcomes for independent memory
evidence, and do not infer general agent usability from either pass^k measure.

## Gates outside this comparator

`compare-paired` does not run an agent, verify whether family labels represent
independent histories, authenticate a model/provider identity, or execute the
existing retrieval-quality and claim-reconciliation profiles. Those controls
must be run against the pinned product binaries and attached to the evaluation
review before making a product claim. A future runner must use the advertised
MCP schema, keep oracle data outside the agent process, record every attempted
turn including failures, and generate both run artifacts from the frozen plan.

The Codex CLI adapter uses the documented `codex exec --json` JSONL events for
turns, MCP calls and usage, and configures its stdio server through per-run
overrides. It does not rewrite the user's persistent Codex configuration or
copy credentials into artifacts. See the official [non-interactive mode
contract](https://developers.openai.com/codex/non-interactive-mode) and
[Codex MCP configuration](https://developers.openai.com/codex/mcp).

**Model identity protocol:** The current official `codex exec --json`
documentation describes the emitted JSONL event types and shows token usage in
`turn.completed`; it does not document a provider-reported immutable model
revision. The earlier local check recorded `codex-cli 0.169.0`; the current
qualification below used `0.171.0`. Both select a model by name, and neither
the requested alias nor the CLI version establishes the backend revision that
served a request. This is a limitation of the documented interface, not proof
that the provider never exposes such identity elsewhere.

The v2 plan makes the distinction explicit with `model_identity_protocol`.
Use `{"method":"provider_revision","revision":"<provider value>"}` only
when the provider reports that revision for every attempt. For the current
Codex CLI path, use
`{"method":"same_alias_within_invocation","max_duration_seconds":7200}`
and pin the requested name in `shared_identity.model`. The runner-generated
artifacts record UTC invocation start and completion times; validation
requires one invocation ID, identical time windows across both arms, and a
window no longer than the preregistered limit. This bounds the comparison in
time but cannot prove that the provider backend stayed unchanged. Any resulting
claim must be limited to this requested alias and observed window, with that
limitation stated. Never encode the alias as a provider revision. A model call
is not a pilot merely because the CLI returns usage and MCP events.

The current ten-case decision corpus is diagnostic development data. It is not
a held-out test set and cannot supply the paired pilot variance or promotion
margins. The one-case Codex CLI connectivity smoke recorded in
[`MEMORY_DECISION_BASELINE.md`](MEMORY_DECISION_BASELINE.md) is transport
evidence only; its model identity and task denominator are insufficient for a
pilot or convenience claim.

## 2026-11-23 local LFM2.5 feasibility screen

The optional `liquid/lfm2.5-1.2b` model was loaded in LM Studio under alias
`memory-local-eval-lfm25` (8-bit, 16,384 runtime context). A synthetic
fake-MCP smoke passed after making the expected citations schema explicit.
The separate frozen four-family paired workflow pilot then completed in
73.88 seconds, but all eight arm receipts ended in `model_failure` and task
oracle success was 0/4 in both arms. Candidate operator imports produced 2/4
independent probe passes while the incumbent had 0/4; this storage-state result
does not offset the failed agent tasks. `compare-agent-managed` accepted the
artifacts with zero hard violations, but the cohort is a feasibility NO-GO for
using LFM as the source-authority agent. The eight agent trial wall times
totalled 5.138 seconds (mean 642 ms); the rest of the process time includes
product/database setup. This is not a matched speed comparison with the
16-family Qwen capture. Keep LFM for fast adapter/tool-call checks; the frozen
Qwen v3 model completed its preregistered run-02 below. Neither pilot
establishes memory treatment benefit or powered agent utility. Registration and
artifacts:
[`source-authority-local-lfm25-pilot-2026-11-23`](../../evals/paired/source-authority-local-lfm25-pilot-2026-11-23/README.md).

## 2026-11-23 local Qwen v3 two-repeat feasibility result

The frozen 16-family Qwen v3 development pilot completed two accepted captures
using the same product binary in both arms. The task oracle passed 0/16 in each
arm in both repeats. Direct-memory success and independent fixed-query probes
were 11/16 incumbent versus 13/16 candidate in both repeats; the paired
direct-memory estimate was +0.125 with a 95% family-cluster interval of
[-0.1875, 0.4375]. There were no hard violations. Candidate task receipts had
two budget exceedances in run-01 and none in run-02; candidate formation had one
budget exceedance in run-02. The same 16 synthetic families were repeated, not
32 independent examples.

This is a feasibility NO-GO for the tested source-authority agent workflow: no
task passed the oracle, and the two arms differ in source preparation rather
than memory implementation. It does not establish a memory treatment effect,
general agent usability or powered benefit. LFM's separate four-family screen
completed in 73.88 seconds but also had 0/4 task-oracle successes per arm, so
its speed does not make it a replacement for agent workflow evaluation; retain
it for quick provider/tool-call smoke only. The frozen artifacts, repeatability
summary and limitations are recorded in the [Qwen v3 run-02 report](../../evals/paired/source-authority-local-chat-completions-v3/runs/run-02/report.md).

In the recorded captures, LFM averaged 0.64 seconds per agent receipt versus
42.2 seconds for Qwen run-02 (about 66x on this observed, non-matched sample).
This ratio is not a model speed benchmark: LFM ran four different families,
all eight LFM receipts failed with normalized `internal` model errors, and the
per-receipt turn/tool mix differed. Qwen also had zero task-oracle successes.
The evidence supports LFM as a fast connection/tool smoke only; neither model
has demonstrated a successful source-authority workflow in these pilots.

## 2026-11-23 local Qwen v4 post-diagnostic qualification

After the runner added granular content-free final-answer classes, a new
four-family Qwen screen was frozen with two conversation and two file-import
families disjoint from v1–v3 and the LFM cohorts. The run used the exact
`memory-local-eval-qwen38` alias and identical product binary in both arms. It
completed once in 11m 32.766s; no raw model response text was stored.

Task-oracle success was 0/4 in each arm, direct-memory success was 3/4 in each,
and independent probes were 3/4 in each. Candidate operator imports and both
source-file probes succeeded, but the agent task still failed on both
file-import cases. There were zero hard violations and one candidate
budget-exceeded task. The final-answer diagnostics observed
`final_answer_not_json` and `invalid_tool_call`; some syntactically completed
answers still failed the exact task oracle. The frozen qualification gate
failed, so the result is a workflow/model NO-GO and does not establish a
memory implementation effect or agent benefit.

The setup attempt that used a relative output directory was rejected before
model process creation and is preserved separately from the valid `run-02`
capture. Full registration, receipts, offline comparison, hashes and limits
are recorded in the [Qwen v4 qualification report](../../evals/paired/source-authority-local-chat-completions-v4-qualification/runs/run-02/report.md).
Do not reuse the exposed families or repeat the same Qwen configuration for a
larger workflow cohort. The open requirement now remains a reliable agent/model
configuration, followed by a fresh preregistered powered comparison.

## 2026-11-23 local Ornith 1.5 qualification

A separate four-family screen tested the already-downloaded
`Ornith-1.5-35B-A3B-MLX-4bit` through the provider-neutral Chat Completions
adapter. Both arms used the same product binary and synthetic histories. Task-
oracle success was 0/4 in each arm; independent probes passed 1/4 incumbent and
3/4 candidate; candidate file-import probes passed 2/2; hard violations were
zero. The recorded formation phase completed in 1/4 cases and ended as
`model_failure` in 3/4 cases per arm. The preregistered screen is therefore
**NO-GO for this tested model/adapter configuration**. It neither isolates a
memory change nor proves a model-wide limitation. The fixed adapter used
temperature 0 and omitted `top_p`, while the [model card](https://huggingface.co/ornith-ai/Ornith-1.5-35B-A3B)
recommends 0.6 / 0.95 for general tasks and documents Qwen3 XML/reasoning
parsers for its serving recipes. Do not scale or reuse this cohort; any sampler
or parser change needs a new frozen registration and disjoint families. The
100.689-second capture is not a controlled speed comparison with the LFM or
Qwen pilots. Full results, hashes and limitations are in the
[run-01 report](../../evals/paired/source-authority-local-ornith-qualification-2026-11-23/runs/run-01/report.md).

This does not change the existing LFM decision: its fast failed receipts make
it useful for transport/tool smoke, not for task-level acceptance. Keep
deterministic evaluation as the fast inner loop and reserve model-mediated
workflow gates for a configuration that first passes its qualification screen.

## 2026-11-23 current Codex source-authority qualification

A frozen 16-family screen used Codex CLI `0.171.0`, requested alias
`gpt-6.1-sol`, synthetic task histories and the same current memory product
binary in both arms. All 16 paired trials completed. The task oracle passed
8/16 incumbent versus 15/16 candidate; independent fixed-query probes passed
16/16 in both arms, candidate source-file tasks passed 8/8, and hard-violation
and budget-exceeded counts were both zero. The candidate/incumbent task-success
difference was +0.4375 with a 95% family-cluster interval of [0.125, 0.75].
By stratum, conversation controls scored 8/8 incumbent and 7/8 candidate;
source-file workflows scored 0/8 incumbent and 8/8 candidate. In the incumbent
file stratum, all task receipts completed but failed both direct-memory and
task-oracle checks, while all eight independent fixed-query probes passed. This
suggests a workflow-specific gap after storage passed its separate probe. A
post-capture contract audit found that incumbent prompts told the agent to pass
the file path as `ingest.content`, contrary to the literal-content MCP contract;
only the explicit operator CLI import reads a file. The source-file incumbent
task therefore encoded unsupported behavior, so its 0/8 is not a valid product
recall failure. See the [contract and import implementation](../../docs/superpowers/specs/2026-11-09-ingestion-source-authority-design.md#contract).

The registered stop gate required task-oracle success of at least 12/16 in
each arm. The incumbent scored 8/16, so this screen is **NO-GO for proceeding
to a broader agent workflow pilot**; the registered second capture was not run.
The candidate's higher scores do not override the stop rule. The two arms used
different source-preparation workflows but the same memory binary, so this
does not establish a memory treatment effect, general usability, non-inferiority
or agent benefit. The 20m 15.861s end-to-end capture is not a controlled speed
benchmark and cannot be compared with LFM's smaller failed cohort. LFM remains
useful for fast transport/tool smoke; deterministic evaluations remain the
fast inner loop. See the [full run report and checksums](../../evals/paired/source-authority-current-codex-qualification-2026-11-23/runs/run-01/report.md).

## 2026-11-23 corrected Codex literal-source qualification

The earlier 16-family Codex screen used an unsupported incumbent instruction:
it passed a file path as the literal `ingest.content` field. That run remains a
NO-GO and is not used to judge source recall. A new frozen registration supplied
the exact source bytes to the incumbent formation prompt and gave the candidate
the same bytes through evaluator-side `import file`, followed by a new read-only
recall session. It used four disjoint synthetic families and the same product
binary/configuration in both arms.

The first process attempt failed before any model turn because Codex could not
initialize its in-process app-server inside the execution sandbox (`Operation
not permitted`); all eight receipts had zero turns/tokens. It is preserved as
invalid [run 01](../../evals/paired/source-authority-codex-literal-input-qualification-2026-11-23/runs/run-01/report.md).
After a one-message synthetic startup smoke outside the sandbox confirmed that
process-permission issue, the frozen qualification ran as [run 02](../../evals/paired/source-authority-codex-literal-input-qualification-2026-11-23/runs/run-02/report.md).

Run 02 completed all four pairs. Both arms scored 1/4 on task-oracle success,
below the preregistered 3/4-per-arm qualification floor. Agent-observed
direct-memory success and independent fixed-query probes were 4/4 in each arm;
candidate operator imports and extraction passed 4/4, with no hard violations
or budget exceedances. The task-oracle pass was the same family in both arms.
These results do not qualify the agent workflow and do not establish a memory
implementation effect: the product binary was identical. The direct-memory
and independent-probe results show that the memory path returned the expected
support, while the exact reasons for the three wrong final answers remain
unknown because raw answer text is not retained. Do not tune or reuse these
exposed families; a later qualification requires a reliable model/configuration,
fresh disjoint families and a new registration.

## 2026-11-23 current Codex v5 diagnostic qualification

A fresh four-family synthetic registration used Codex CLI `0.171.0`, the
existing local login, and the same product binary in both arms. A separate
restricted-executor attempt failed before any model turn (zero turns, tokens
and tool calls); it is preserved as invalid preflight evidence and is not a
model result. Its exact process failure is unknown because stderr was not
retained.

The valid disjoint capture completed all eight arm trials in about 4m52s.
Task-oracle success was 1/4 per arm, below the frozen 3/4-per-arm qualification
floor. Direct-memory evidence and independent fixed-query probes were 4/4 in
both arms. Final answers were present, schema-valid, included required evidence
and matched clarification behavior in 4/4 per arm; accepted-answer match was
1/4. Candidate imports and extraction passed 4/4, with no hard violations or
budget exceedances. The answer-value misses are visible in v5 diagnostics, but
raw answers and retrieved passages were not retained, so their cause cannot be
assigned to the model or memory. Both arms used the same memory binary; this
does not measure a memory treatment effect or establish agent benefit. The
screen is **NO-GO for this tested workflow configuration**. Do not repeat or
tune this exposed cohort. The [valid run report and checksums](../../evals/paired/source-authority-codex-literal-input-diagnostic-escalated-2026-11-23/runs/run-01/report.md)
and [invalid preflight report](../../evals/paired/source-authority-codex-literal-input-diagnostic-2026-11-23/runs/run-01/report.md)
preserve the outcomes.

## 2026-11-23 Qwen speed-optimized source-authority screen

A fresh four-family cohort used the local Qwen3.8-27B speed-optimized alias
with the provider-neutral Chat Completions runner. The offline comparator
accepted the artifacts, and an independent replay was byte-identical. Task
oracle success was 0/4 in both arms, below the frozen 3/4-per-arm gate.
Independent probes passed 1/4 incumbent versus 4/4 candidate; candidate
source-file task success was 0/2. Both candidate imports/extractions passed,
with zero hard violations or budget overruns. Candidate answers were all
present, schema-valid and evidence-citing but accepted-answer match was 0/4.

This is **NO-GO for the exact configuration** and does not qualify the workflow
for a powered follow-up. Both arms used the same memory binary, so it is not a
memory effect. The capture took 7m11.721s, but separate cohorts are not a
controlled model-speed comparison. Preserve the exposed families and do not
tune or rerun them. See the [frozen registration](../../evals/paired/source-authority-local-qwen-speed-v1-qualification/README.md),
[run report](../../evals/paired/source-authority-local-qwen-speed-v1-qualification/runs/run-01/report.md),
and output [checksums](../../evals/paired/source-authority-local-qwen-speed-v1-qualification/runs/run-01/SHA256SUMS).

## 2026-11-24 Inkling 2 Mini runtime preflight

The official [MLX-VLM v0.8.1 release](https://github.com/Neywa-Labs/mlx-vlm/releases/tag/v0.8.1),
published 2026-11-23, adds Inkling 2 Mini model and tool-parser support. To
recheck the earlier LM Studio failure without changing its runtime, MLX-VLM
0.8.1 was installed in a temporary Python 3.12 environment and pointed at the
already downloaded `mlx-community/Inkling-2-Mini-oQ4e-mtp` weights. The model
loader rejected 1,096 checkpoint parameters as unexpected before inference.
The [checkpoint card](https://huggingface.co/mlx-community/Inkling-2-Mini-oQ4e-mtp)
describes this oQ4e-MTP artifact for oMLX Lightning MTP, so this establishes a
runtime/artifact compatibility failure for this local variant, not a failure
of Inkling 2 Mini generally. No token, MCP call, paired trial or quality score
was produced; no qualification cohort was created. An original upstream
checkpoint or a runtime supporting this exact quantized artifact would require
a fresh compatibility preflight before any new registration.

## 2026-11-24 corrected-sampler Ornith source-authority screen

A fresh four-family screen used Ornith 1.5 35B A3B, the existing literal-input
protocol, and the model-card sampling values `temperature=0.6`, `top_p=0.95`,
`top_k=20`. The frozen inputs and family separation were verified before the
single capture. The run took 114.026 seconds and the offline comparator
accepted the artifacts; a separate comparator replay was byte-identical.
Task-oracle success was 0/4 incumbent and 2/4 candidate, below the 3/4 floor
for each arm. Independent probes passed 1/4 and 3/4, below the 4/4 floor.
Candidate source-file tasks and operator imports/extractions passed 2/2;
hard violations and budget overruns were zero.

The paired task-success estimate was +0.50 with a descriptive family-cluster
95% interval [0.00, 1.00] on four synthetic families. Both arms used the same
product binary and config, so this is not a memory effect. Formation sidecars
recorded three `transport` failures per arm. That normalized class covers
client send failures, non-success HTTP responses, and response-body read
failures; the specific status and body were not retained, so the underlying
cause is unknown. The failed frozen quality gate is a NO-GO for this exact
model/runner/sampler/workflow. It does not establish that Ornith itself is
incapable or that memory is defective. Preserve the cohort; do not tune or
rerun it. See the [registration](../../evals/paired/source-authority-local-ornith-corrected-sampler-qualification-2026-11-24/README.md),
[pre-capture audit](../../evals/paired/source-authority-local-ornith-corrected-sampler-qualification-2026-11-24/pre-capture-audit.md),
[run report](../../evals/paired/source-authority-local-ornith-corrected-sampler-qualification-2026-11-24/runs/run-01/report.md),
and [output checksums](../../evals/paired/source-authority-local-ornith-corrected-sampler-qualification-2026-11-24/runs/run-01/SHA256SUMS).

## LFM2.5 speed-up decision

The frozen LFM2.5-1.2B pilot and a disjoint diagnostic both scored 0/4
task-oracle successes per arm. In the pilot, model calls took 5.138 seconds of
73.88 seconds total; this is not a matched timing comparison, but it shows
that end-to-end runner and product setup dominate a small four-family run.
The [Liquid model card](https://huggingface.co/LiquidAI/LFM2.5-1.2B-Instruct)
recommends the model for agentic tasks, and documents that its default function
calls use a Pythonic special-token format; JSON function calls can be requested
in the system prompt. [LM Studio's tool-use documentation](https://lmstudio.ai/docs/developer/openai-compat/tools)
also says unparseable calls may be returned as ordinary content instead of
`tool_calls`. This leaves a possible model-specific prompting/parser issue,
but the existing results do not show useful quality as a workflow evaluator.
Keep LFM for quick transport/tool smoke; any new agent qualification needs a
specific technical hypothesis, a fresh disjoint registration, and frozen
acceptance thresholds. Do not use its speed to replace quality checks.
