# Agent-managed formation and recall pilot preregistration

**Frozen:** 2026-11-22, before any Codex task invocation.  
**Plan:** `plan.json` (`memory-mcp-agent-plan/v2`, `development_pilot`).

## Question and limits

Can the optional eval runner safely measure agent-managed memory formation, fresh-session recall, and post-write retrieval while keeping storage isolated and evaluator evidence hidden? This is an instrumentation and feasibility pilot on eight new, independent, synthetic fact families. It is not a powered estimate of agent utility, a promotion test, a test of real-world workflow usability, or a product-memory quality claim.

## Frozen protocol

- Each family starts with an empty, isolated embedded store. A Codex formation session receives exactly one synthetic durable fact and may call only `assemble_context`, `ingest`, `extract`, and `invalidate`. A separate fresh Codex process recalls from the same trial store and may call only `assemble_context`. The evaluator then runs a fixed query directly against the store; this probe has no agent session.
- The scorer/oracle stays in the evaluator process. A target source is credited only when exactly one successful formation ingest and its returned episode ID are observed. Raw memory payloads are not retained in trace artifacts.
- One registered paired invocation covers all eight families; arm order alternates by preregistered key. Run a second separate invocation with the identical plan/tasks/products to report repeatability. These are same-product retests, not incumbent/candidate treatment effects.
- One attempt per arm and family; no runner retries. Each Codex phase is capped at 120 seconds, eight turns, eight MCP calls, and 2,048 output tokens. No shell, web, plugins, apps, memories, or permission requests are enabled. Codex login is read from the user's existing local Codex home; no credential is copied into the repository or artifacts.

## Endpoints

Primary descriptive formation/resource endpoints, reported per arm and per invocation: formation completion/status, turns, tool calls, clarification count, input/output tokens, wall time, write attempts/successes/failures/unresolved by tool, and retained store bytes. Task success and source support in the fresh recall, plus the evaluator's independent fixed-probe source support, are safety/feasibility checks. Report family-level counts and both invocation results; do not fit a promotion margin or claim statistical power from eight families.

## Stop and validity rules

Abort and retain an invalid preflight record if any registered binary, Codex version, prompts, protocol config, task/oracle hash, tool schema, launch config, or auth source differs from `plan.json`; if a trial store is reused across arms; if the oracle becomes visible to Codex; or if any unplanned tool is reachable. Do not silently retry failed trials or change tasks/prompts after starting. A failed/timeout task remains in the denominator. Any code or protocol change after the first invocation requires a new plan ID and a fresh run set.

## Frozen identities

See `plan.json`, `tasks.json`, `host-policy.json`, `retry-policy.json`, `simulator.json`, `product-config.json`, and `protocol-config.json`. The Codex provider is identified by executable SHA-256 and version; the model is pinned to the `gpt-6.1-sol` alias within each invocation. Claims across invocations are descriptive because alias resolution may change.

Product configuration identity is SHA-256 over compact UTF-8 JSON with sorted keys for the five public server settings in `product-config.json`; path-specific store directories, credentials, and the separately pinned claim stage are excluded. Corpus and oracle fingerprints use the evaluator canonical serialization and are emitted by `hash-agent-managed-tasks`. File SHA-256 values are recorded separately in `SHA256SUMS.txt`.

## Preregistration amendment v5

V4 is retained as a valid descriptive diagnostic capture, but both complete repeats failed formation: the broad formation tool surface caused duplicate ingest/extract attempts and exceeded the preregistered 65,536-token input ceiling in all 16 arm/family trials. The independent probes consequently had no attributable writer episode IDs. V5 tests a changed, least-privilege formation protocol in a new development pilot; it is not a continuation, confirmation, or promotion test. It uses eight disjoint synthetic histories (`tasks-v5.json`), one fresh store per arm/family, exactly `ingest` + `extract` as the formation allow-list, and the existing fresh read-only recall session plus direct fixed evaluator probe. The formation system/task prompts require a single ingest and extraction of its returned ID; the harness reads the MCP JSON text wrapper for that opaque ID and does not retain source content.

V5 preserves the frozen 120-second phase, eight-turn, eight-tool-call, 2,048-output-token, and 65,536-input-token limits. No retries, unplanned tools, shell, web, or other Codex features are enabled. Same alias/model and current local ChatGPT login are pinned only within each invocation; cross-invocation results remain descriptive. `plan.json`, `plan-v5.json`, `fingerprints.json`, and `SHA256SUMS.txt` freeze the exact evaluator binary, product binary, Codex CLI, tasks/oracle, host policy, protocol and phase prompts before either v5 invocation. Any identity mismatch aborts before a task; any v5 trial failure remains in its denominator. V4 artifacts are unchanged and are not pooled with V5.


## Preregistration amendment v2

The v1 capture is invalid because an evaluator-side sorted-key comparison rejected a valid alternating manifest after execution. The harness fix is protected by a regression that first failed and then passed. No v1 outcome is scored. This v2 plan pins the rebuilt `memory-eval` executable in `shared_identity.provider_fingerprints.memory_eval_cli` and pins the task manifest, host/retry/simulator policies, protocol config and product independently. The full v2 run set is fresh and uses the exact same frozen cases and prompts.


## Preregistration amendment v3

The v2 paired capture is invalid because the outer restricted execution layer prevented Codex from initializing its local app-server before any model turn. This was confirmed by an isolated no-memory preflight: the same pinned CLI/model and read-only Codex settings fail inside the restricted execution layer and complete when launched directly on the host. That one operational preflight is excluded from all outcomes and had no MCP server or memory access. The runner is now executed directly on the user's laptop as explicitly authorized; the Codex process itself remains in its read-only sandbox with shell, web, plugins, apps, and memory features disabled. This host-launch mode is pinned in `host-policy.json`. No v2 trial outcomes enter the v3 analysis.


## Preregistration amendment v4

The v3 attempt reached 15/16 arm trials, then evaluator validation rejected a legitimate negative probe with one unmapped source and zero known sources. This was a validator defect: unsupported-source count is independent of the number of known source event IDs. A new public runner regression reproduced the rejection, then passed after relaxing only that incorrect relation; the probe remains a failure unless the required mapped evidence is present and no unsupported sources exist. The v3 artifacts are excluded. V4 pins the rebuilt evaluator binary and reruns both complete paired captures on fresh stores.
