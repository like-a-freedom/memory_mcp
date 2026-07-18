# Agent Memory Discipline — Architecture and Implementation Plan

**Status:** Proposed, critically revised on 2026-07-18  
**Deliverable type:** Architecture and implementation plan only; no implementation is included  
**Scope:** Make agents read from and write to memory continuously and automatically, primarily through MCP, without relying on the model to remember the workflow  
**Research cutoff:** 2026-07-18  
**Related design:** docs/CONTRADICTION_DETECTION_DESIGN.md, ADR-0002 through ADR-0015, and docs/superpowers/plans/2026-07-18-claim-reconciliation.md  
**Supersedes:** the previous version of this file and docs/papers/Механизм взаимодействия memory_mcp с AI-агентами  план внедрения.md

---

## 1. Decision summary

The goal cannot be met by an MCP server, tool descriptions, or an agent prompt alone.

MCP tools are selected by the model. A server may describe the correct workflow, but it cannot force the first tool call or issue an unsolicited reminder after the client has gone idle. Therefore, reliable memory discipline requires two cooperating control planes:

1. **The host or agent harness owns enforcement.** A lifecycle bridge invokes memory at session, prompt, tool, compaction, and task boundaries. This is the only layer that can make calls happen independently of model attention.
2. **memory_mcp owns memory capability and policy.** It exposes compact intent-level MCP operations, validates writes, retrieves current and auditable context, persists raw evidence, and performs durable background processing.

The target design adds two agent-facing capability operations:

- **memory_prepare_task** — one read-oriented context call that returns task-relevant current facts, recent state, unresolved disagreements, provenance handles, and a context receipt.
- **memory_record_event** — one idempotent write call that persists a significant event and schedules derived processing without requiring the agent to orchestrate ingest then extract.

The existing tools remain available as the product/compatibility layer. They are corrected and documented accurately, but ordinary agents should not need to compose them for the common read-before-act and write-after-salience paths.

The complete lifecycle is:

    user or environment event
        -> host lifecycle bridge
        -> memory_prepare_task
        -> memory context injected as untrusted data
        -> model plans or acts
        -> verified user/tool/agent event
        -> memory_record_event
        -> durable raw episode + durable work item
        -> background extraction, linking, reconciliation, and consolidation
        -> future memory_prepare_task

This design deliberately separates:

- automatic invocation from model-selected tool use;
- foreground durability from background intelligence;
- source facts from derived claims;
- claim supersession/correction from source-fact retraction;
- ordinary historical invalidation from privacy erasure;
- memory exposure from proof that the agent actually used the memory.

---

## 2. Critical review of the previous plan

The previous plan identified the central server-versus-harness boundary correctly, but several conclusions are now stale or technically wrong.

### 2.1 What remains valid

- A pure MCP server cannot force the model to make the first memory call.
- Server instructions and tool descriptions are useful but probabilistic.
- Host integrations are needed for reliable initiation.
- Raw evidence, provenance, temporal validity, and contradiction handling are core requirements.
- Heavy consolidation should not be placed on every conversational hot path.
- Discipline needs an evaluation harness based on real multi-turn agent behavior.
- A bare-MCP connection must be treated as a degraded mode, not as equivalent to a lifecycle integration.

### 2.2 Corrections required

| Previous claim or design | Current evidence | Correction |
|---|---|---|
| Codex has no hook model | The locally installed Codex CLI 0.145.0-alpha.18 reports stable hooks; current official documentation supports SessionStart, PreToolUse, PostToolUse, and PermissionRequest, while UserPromptSubmit and Stop are not supported | Add a Codex adapter with an event-specific contract; do not copy the Claude hook map |
| Tool descriptions are missing “when to use” guidance | Current descriptions already contain “Use this tool when…” clauses | Treat description work as contract correction and conformance, not as a greenfield rewrite |
| Tool-result guidance is the strongest universal sustainment mechanism | Current chains are mostly ingest -> extract and assemble_context -> explain; they do not ensure the next task recall or a later salient write | Keep guidance, but do not count it as lifecycle enforcement |
| resolve is read-only | Entity resolution may add an alias or create a new entity | Never set readOnlyHint on resolve; document the mutation |
| The write path should normally call ingest then extract | Inline extract already performs ingest then extract | Common writes need one capability call; the current inline path still lacks project, visibility, policy, and stable event semantics |
| Inline extract is naturally idempotent | Its default source ID is a content hash, but the deterministic episode ID also includes t_ref; defaulting t_ref to now can create another episode on retry | Require a stable event ID and reference time at the capability boundary |
| Contradiction warnings need conflicting_fact_id added | The current ContradictionWarning already includes it | Do not plan an already-complete field |
| A contradiction should cause invalidate on the old fact | Accepted ADRs distinguish claim contradiction/supersession/correction from source-fact retraction | Routine updates must flow through claim reconciliation; invalidate remains source-fact retraction |
| invalidate means “outdated or superseded” | This conflicts with ADR-0002, ADR-0009, and ADR-0015 | Rewrite the public description and integration guidance before enforcing writes |
| explain can prove that an answer used memory | Current access_count and last_accessed only show retrieval/access; they cannot establish causal use in a response or tool call | Add context receipts and host action correlation; treat agent-declared used facts as a claim, not proof |
| A two-call wake_up + timeline bootstrap is an acceptable default | It increases the chance of a broken chain and wake_up ignores the task query | Replace it with one task-level capability call implemented over existing service primitives |
| Hooks can simply “call MCP” | A command hook usually runs outside the host’s in-process MCP channel | Provide a bridge transport: persistent MCP when available, otherwise the existing one-shot CLI backed by the same service layer |
| Fixed quality targets can be declared before a corpus exists | Current numbers are not grounded in a discipline baseline | Define deterministic integration SLOs now; set empirical quality thresholds after baseline measurement |
| A specialized Memory Agent can own required memory calls | Delegation is still model/harness controlled and can be skipped | Required pre-read and post-write stay in the host bridge; a Memory Agent is optional for review or analysis only |

### 2.3 Current repository facts the plan must respect

- The public memory primitives are **ingest**, **extract**, **resolve**, **assemble_context**, **explain**, and **invalidate**. With MCP Apps enabled, **open_app** and **app_command** add an interactive layer.
- The CLI already exposes one-shot equivalents for all six memory primitives. This is the correct fallback transport for command hooks that cannot re-enter the host’s MCP session.
- Inline **extract** already composes ingestion and extraction, but does not currently preserve all ingestion metadata needed for disciplined project-scoped capture.
- **resolve** is resolve-or-create and can persist aliases/entities.
- **assemble_context** already records aggregate access heat, but no retrieval receipt connects a particular context pack to a later answer or action.
- The current MCP TaskStore is process-local server state. It is not a durable consolidation queue.
- Decay, archival, and community workers exist but are disabled by default. They do not implement full semantic reflection.
- Claim reconciliation has a separate accepted design and implementation plan. This plan must consume that subsystem, not duplicate or weaken it.

---

## 3. Evidence from research

### 3.1 Scientific findings that constrain the architecture

| Evidence | Finding | Design consequence |
|---|---|---|
| [Generative Agents](https://arxiv.org/abs/2304.03442) | Useful memory combines relevance, recency, and importance; reflection and planning add value beyond raw recall | Keep raw episodes, add ranked retrieval, and perform derived consolidation outside the hot path |
| [MemGPT](https://arxiv.org/abs/2310.08560) | Long-running agents benefit from a small active context plus external archival memory | Return a bounded task pack rather than flooding the model with history |
| [LongMemEval](https://arxiv.org/abs/2410.10813) | Long-term memory requires information extraction, multi-session reasoning, temporal reasoning, updates, and abstention | Evaluate the complete lifecycle and preserve evidence for temporal/current-state queries |
| [MemBench](https://aclanthology.org/2025.findings-acl.989/) | Factual and reflective memory need accuracy, efficiency, latency, and capacity measures | Do not reduce success to retrieval recall |
| [A-MEM](https://arxiv.org/abs/2502.12110) | Atomic notes and dynamically evolving links can outperform a flat memory store | Derived notes and links are useful, but must remain traceable to immutable sources |
| [MemoryAgentBench](https://arxiv.org/abs/2507.05257) | Accurate retrieval, test-time learning, long-range understanding, and selective forgetting are distinct competencies | Test each competence separately; no single aggregate QA score is sufficient |
| [MemFail](https://arxiv.org/abs/2605.26667) | Summarization, storage, and retrieval have different failure signatures; more retrieved tokens or a stronger model may not fix architectural failures | Preserve raw evidence and instrument each pipeline stage independently |
| [MemoryArena](https://arxiv.org/abs/2602.16313) | Systems that perform well on conversational recall can still fail multi-session action tasks | Evaluate action -> feedback -> memory -> later action loops |
| [EvoMemBench](https://arxiv.org/abs/2605.18421) | No memory form wins across all knowledge and execution settings; long context remains competitive in some regimes | Use task-adaptive retrieval and keep an explicit no-memory/long-context baseline |
| [Mem2ActBench](https://aclanthology.org/2026.acl-long.370/) | Seven evaluated frameworks remain weak at using memory to select tools and ground parameters | Measure action grounding, not only whether recall was called |
| [LightMem](https://aclanthology.org/2026.acl-long.588/) | Separating online retrieval/write from offline consolidation improves the accuracy-efficiency trade-off | Foreground capture must be bounded; durable background work is a first-class subsystem |
| [AgeMem](https://aclanthology.org/2026.acl-long.981/) | Store, retrieve, update, summarize, and discard can be learned as agent actions | Do not assume an untrained general model will reproduce a policy that required dedicated RL |
| [Memora / FAMA](https://aclanthology.org/2026.findings-acl.1337/) | Agents frequently reuse obsolete memories; evaluation should penalize invalid-memory influence | Add stale-influence metrics and deterministic current-truth assembly |
| [LongMemEval-V2](https://arxiv.org/abs/2605.12493) | Procedural/runbook memory improves experienced task performance but can be expensive | Separate factual recall from reusable procedural experience and gate promotion |
| [Memory poisoning study / MPBench](https://arxiv.org/abs/2606.04329) | Aggressive write and retrieval policies increase poisoning risk; prompt-injection filters alone are insufficient | Enforce source-aware write policy, quarantine, provenance inheritance, and security evaluation |

### 3.2 Synthesis of scientific best practices

The architecture should:

1. Preserve a provenance-bearing raw episode for every durable memory.
2. Build atomic facts, claims, summaries, links, and procedural lessons as derived projections.
3. Keep working/task state, episodic history, semantic facts, preferences, and procedural experience distinguishable.
4. Use hybrid retrieval: exact/lexical, semantic, entity/graph, temporal, and policy filters.
5. Apply deterministic time/version rules where the domain permits them; do not ask an LLM to be the sole freshness arbiter.
6. Return timestamps, confidence, rationale, provenance, and an explicit insufficient-support state.
7. Separate bounded online capture/retrieval from durable offline extraction and consolidation.
8. Evaluate whether memory changes actions and parameters, not merely whether a fact was retrieved.
9. Preserve coexisting facts and avoid treating all same-topic differences as contradictions.
10. Treat persistent memory as a security boundary with explicit origin and trust.
11. Distinguish historical invalidation from privacy deletion/redaction.
12. Compare against no-memory, long-context, and simple RAG baselines.

---

## 4. Evidence from analogous projects

Project documentation and repositories are useful for implementation patterns, not as directly comparable scientific benchmarks. Vendor-reported metrics use different datasets, splits, readers, and definitions.

| Project | Relevant mechanism | What to adopt | What not to copy |
|---|---|---|---|
| [Mem0](https://github.com/mem0ai/mem0) | Explicit search-before-answer and add-after-turn integrations; hybrid/vector/graph approaches | Clear integration lifecycle and compact high-level calls | ADD-only behavior as a complete update model; raw CRUD-style MCP responses |
| [Cognee](https://github.com/topoteretes/cognee) | remember/recall/forget/improve; session cache; Claude plugin using SessionStart, UserPromptSubmit, PostToolUse, PreCompact, and SessionEnd | A real host lifecycle bridge, background sync, and four intent-shaped verbs | Assuming hook behavior is portable across clients |
| [Engram](https://github.com/Gentleman-Programming/engram) | Session lifecycle tools, explicit Memory Protocol, compaction recovery, project detection, hooks/plugins plus bare-MCP fallback | Project resolution, compaction recovery, client-specific install profiles, honest bare-MCP degradation | Nineteen agent-facing tools and overlapping session/search utilities |
| [CocoIndex](https://github.com/cocoindex-io/cocoindex) | Delta-only recomputation, input/code fingerprints, lineage, retries, failure isolation | Incremental derived projections, reproducible fingerprints, durable retry/DLQ semantics | Treating an indexing engine as the full agent memory discipline |
| [MentisDB](https://github.com/CloudLLM-ai/mentisdb) | Append-only thought chains, typed relations, checkpoints, ranked/context-bundle retrieval, versioned sidecars | Tamper evidence, explicit freshness, checkpoint recovery, separable indexes | Large public tool surface and self-reported quality claims without local reproduction |
| [MenteDB](https://github.com/nambok/mentedb) | Write-time quality, dedup, bi-temporal records, ACLs, delta serving, multi-pass retrieval, consolidation modules | Quality gates, delta context, source isolation, multi-pass retrieval | Thirty-two MCP tools and synchronous heavy intelligence on every write |
| [MinnsDB](https://github.com/Minns-ai/MinnsDB) | Temporal graph/tables, ontology-driven cardinality, supersession, event triggers | Typed temporal transitions and event-driven processing | Treating WIP memory formation as proven or using ontology inference without review gates |
| [MemPalace](https://github.com/mempalace/mempalace) | Verbatim history, scoped hierarchy, wake-up, periodic/pre-compaction hooks, temporal KG | Raw evidence retention, checkpoints, scoped retrieval | Twenty-nine agent-facing tools and retrieval-only memory as a complete semantic/procedural solution |
| [Graphiti](https://github.com/getzep/graphiti) | Immutable episodes, temporal entity/fact graph, provenance, valid/invalid intervals, hybrid search | Temporal provenance and incremental graph updates | Assuming a graph alone guarantees correct or proactive use |
| [Letta / MemGPT](https://github.com/letta-ai/letta) | Stateful runtime controls the model loop and memory blocks | Small always-on core plus external memory | Turning memory_mcp into a complete agent runtime |
| [LangMem](https://github.com/langchain-ai/langmem) | Hot-path search/manage tools plus a background manager | Clear foreground/background boundary | Binding the server architecture to one agent framework |
| [Hindsight](https://github.com/vectorize-io/hindsight) | retain/recall/reflect and a wrapper around model calls | Automatic wrapper/middleware as the reliability layer | Treating vendor benchmark claims as release evidence |

The strongest shared operational pattern is not a particular database. It is:

- a small intent-level API;
- a host/plugin/wrapper that owns lifecycle calls;
- a durable raw record;
- derived and incremental processing;
- explicit compaction/session recovery;
- observability and user control.

### 4.1 Community evidence

Community discussions repeatedly report stale facts, near-duplicates, noisy extraction, lost rationale, weak inspectability, and agents that can search memory but do not use it in their actions. These are useful failure hypotheses, not comparative evidence. The plan therefore converts them into test cases rather than quoting anecdotal accuracy numbers.

The supplied Perplexity page was reachable but its content could not be extracted reliably. No claim in this plan depends on it.

---

## 5. Target responsibility model

### 5.1 Three levels of reliability

| Level | Owner | Mechanism | Reliability |
|---|---|---|---|
| **Host-enforced** | Lifecycle bridge or custom harness | Calls memory independently of model choice | Required for guarantees |
| **Protocol-assisted** | memory_mcp | Intent tools, server instructions, annotations, guidance, resources | Improves selection and recovery but cannot initiate reliably |
| **Model-opportunistic** | Model | Additional recall, explain, or capture calls | Useful supplement, never the enforcement boundary |

Only the first level may be described as automatic or mandatory.

### 5.2 Two meanings of “background”

The design uses the word background only for two explicit mechanisms:

1. **Host-triggered automatic calls** that happen at lifecycle boundaries without requiring the model to choose a tool.
2. **Server-side durable jobs** that process an already-persisted episode after the foreground call returns.

MCP Tasks may expose status for long-running client-requested work, but they are not a scheduler and do not authorize unsolicited server work. A durable internal queue is required for extraction and consolidation that must survive client disconnects or process restarts.

### 5.3 Required lifecycle

| Boundary | Required behavior |
|---|---|
| Session start/resume | Load a small orientation pack: recent task state, constraints, decisions, commitments, and unresolved relations |
| Before a user prompt is processed | Retrieve focused context for that prompt where the host exposes a pre-prompt hook |
| Before a significant or high-risk tool action | Retrieve file/module/task-specific facts and policy constraints |
| After a tool action | Capture the verified outcome, including failure, outputs, and affected targets |
| After an explicit decision, preference, correction, or commitment | Capture a structured significant event |
| Before compaction | Persist a checkpoint with current state, unresolved work, and relevant receipts |
| Session/task end | Persist outcomes, failures, unresolved blockers, and lesson candidates; flush durable writes |
| Background | Extract, link, reconcile, index, consolidate, archive, and report failures |

---

## 6. Architectural decisions

### AD-1 — Enforcement belongs to the host lifecycle bridge

**Decision:** Required memory calls are made by host hooks or harness middleware. MCP remains the primary agent protocol, but it is not the enforcement mechanism.

**Why:** Tool invocation is model-controlled. Server instructions, descriptions, resources, and guidance cannot guarantee the first call.

**Consequence:** Every supported host needs an explicit capability matrix and integration tests. Bare MCP is a documented degraded mode.

### AD-2 — Add a compact capability facade; keep existing primitives

**Decision:** Add **memory_prepare_task** and **memory_record_event** as the default agent profile. Keep ingest, extract, resolve, assemble_context, explain, and invalidate for compatibility, advanced workflows, CLI use, and diagnostics.

**Why:** The current common paths require multi-step orchestration or use names that describe storage mechanics rather than agent intent. Two capability calls reduce partial workflows without creating a large tool surface.

**Consequence:** The agent-facing profile should normally expose the two capability tools plus explain and explicit retraction only where needed. Apps remain a separate interactive profile.

### AD-3 — Foreground durability, background derivation

**Decision:** memory_record_event validates policy, writes the immutable raw episode, and creates a durable work item atomically. Extraction and consolidation may complete asynchronously.

**Why:** A significant event must not be lost because NER, embeddings, or reconciliation are slow or unavailable.

**Consequence:** Responses distinguish stored, duplicate, quarantined, rejected, queued, partial, and projected states. Queue lag and failures are observable.

### AD-4 — Source facts and claim lifecycle remain separate

**Decision:** Consume the accepted claim-reconciliation design:

- contradiction records a relation and does not retract either source fact;
- supersession closes claim validity only with sufficient temporal/source evidence;
- correction changes transaction-valid derived representation for the same validity context;
- invalidate retracts an erroneous, withdrawn, corrupted, or incorrectly ingested source fact;
- privacy erasure/redaction is a separate administrative operation.

**Why:** Reusing fact invalidation for normal updates destroys provenance and conflicts with accepted ADRs.

**Consequence:** The current invalidate description and agent integration text must be corrected before automatic capture is enabled.

### AD-5 — Every write carries origin and trust

**Decision:** Durable capture includes explicit source_kind, actor, trust_class, scope, project, policy tags, session/task/event identity, t_ref, and t_ingested. Trust is inherited by derived facts, claims, summaries, and lessons.

**Why:** Automatic memory is a persistent attack surface. User statements, tool results, external pages, repository files, and model inferences are not equally authoritative.

**Consequence:** External untrusted content cannot become a user preference, security policy, or procedural lesson without confirmation or an authoritative connector policy.

### AD-6 — Keep retrieved memory in a data boundary

**Decision:** Context packs explicitly mark memory as evidence, not instructions. Source and trust labels are preserved in the model-facing pack. High-impact actions require live verification when support comes only from low-trust memory.

**Why:** Memory poisoning can turn a single malicious write into repeated future influence.

**Consequence:** Prompt templates, response schemas, and security tests must preserve instruction/data separation.

### AD-7 — Record exposure with a context receipt

**Decision:** Every memory_prepare_task call receives a stable host request ID and returns a context_receipt_id covering query, scope, policy version, retrieved fact/claim IDs, relation state, ranking mode, timestamps, and latency. Later action/outcome events may reference it.

**Why:** Aggregate access heat cannot show which context informed which action.

**Consequence:** Telemetry can prove that memory was exposed before an action. It still cannot prove causal model use; that needs action-grounding evaluation or explicit citations. Persisting the receipt is a logical audit write, so the MCP operation must not claim readOnlyHint=true unless receipts become stateless and all access-side effects are removed.

### AD-8 — Client adapters share policy, not identical hooks

**Decision:** A common bridge core owns trigger policy, schemas, idempotency, retries, and telemetry. Thin client adapters map actual host events to that core.

**Why:** Claude Code, Codex, OpenCode, Gemini CLI, IDEs, and custom harnesses expose different lifecycle events.

**Consequence:** Integration documentation must be generated or tested from one canonical discipline contract, while event mappings remain client-specific.

### AD-9 — Degrade visibly and according to risk

**Decision:** A failed memory call always emits a degraded-mode event. Ordinary work may fail open with a visible warning; high-risk actions configured to require memory freshness fail closed or require user confirmation.

**Why:** Silently continuing defeats auditability, while blocking all work makes the integration unusable.

**Consequence:** The bridge policy defines risk classes and fallback behavior rather than leaving it to each hook script.

### AD-10 — Procedural knowledge requires stronger promotion evidence

**Decision:** A single successful-looking trace is a lesson candidate, not a durable procedure. Promotion requires explicit user confirmation, an authoritative source, or repeated verified success without contradictory outcomes.

**Why:** False-precedent and skill-procedure poisoning are amplified by self-improvement loops.

**Consequence:** Procedural memory has its own trust, support count, evaluator version, and rollback history.

### AD-11 — Evaluation and rollout are architecture components

**Decision:** Build trace capture, an oracle-labeled corpus, and failure-mode tests before making lifecycle enforcement default.

**Why:** Aggressive capture can improve recall while degrading precision, security, latency, and action quality.

**Consequence:** Rollout progresses through observe-only, shadow, opt-in enforced, and default-on gates.

---

## 7. Agent-facing MCP design

### 7.1 Server profile

**Domain:** Durable, provenance-aware memory for long-running agents  
**Persona:** Coding, knowledge-work, and custom agents operating across sessions  
**Layer:** Capability facade over the existing product primitives

### 7.2 Tool: memory_prepare_task

**Intent:** Give the agent the smallest decision-ready memory pack required to answer, plan, or act on the current task.

**Description contract:**

> Prepare long-term memory for the current task in one read-oriented context call.
>
> Use this tool before answering a memory-dependent prompt, planning non-trivial work, resuming a task, or performing a significant action. Host integrations should call it automatically at supported lifecycle boundaries.
>
> Do not use it to create or modify domain memory or to retrieve raw history without a task. The operation records only its immutable audit receipt/access telemetry. Use explain only when full source evidence is needed beyond the included provenance handles.
>
> Returns current task-relevant facts, recent task state, unresolved contradiction/ambiguity summaries, provenance handles, an insufficient-support indicator, and context_receipt_id.

**Flat arguments:**

| Argument | Required | Notes |
|---|---|---|
| request_id | yes | Stable host/agent idempotency key for this lifecycle event and receipt |
| task | yes | Current user request, action intent, or resume objective |
| scope | yes | personal, team, org, or private-domain |
| project | no | Restricts all retrieval and relation enrichment |
| session_id | no | Correlates reads with a host session |
| task_id | no | Correlates reads across sessions |
| as_of | no | Point-in-time query |
| budget | no | Bounded fact/context budget |
| trigger | no | session_start, prompt, pre_action, resume, context_switch, or manual |
| risk_level | no | normal or high; high requests stronger freshness/trust signaling |

**Decision-ready return:**

- status and guidance;
- context_receipt_id;
- model_context;
- current facts with confidence, validity, trust, and provenance handles;
- recent task state and unresolved commitments;
- active contradiction/temporal-ambiguity summaries with counterpart handles;
- insufficient_support and degraded_components;
- retrieval policy/evaluator versions;
- pagination only if the requested budget is intentionally exceeded.

**Implementation note:** This is a service-level composition over current retrieval, wake-up/recent state, claim-relation enrichment, and minimal provenance. It does not require a new semantic-memory model or a new LLM, but the chosen durable-receipt design does require a small append-only audit record and index.

**Idempotency rule:** Repeating the same request_id with the same scope/project/task identity returns the original receipt and context snapshot. Reusing it with conflicting identity fails loudly. A deliberate refresh uses a new request_id.

### 7.3 Tool: memory_record_event

**Intent:** Durably capture one significant event and schedule safe derived memory processing in a single idempotent call.

**Description contract:**

> Record a significant, source-attributed event in long-term memory.
>
> Use this tool after an explicit user preference or constraint, decision with rationale, commitment, correction, verified tool outcome, task checkpoint, failure with root cause, or completed task outcome. Host integrations should call it automatically for verified lifecycle events.
>
> Do not send an entire conversation, untrusted external instructions, secrets, small talk, or unsupported model guesses. External content must keep source_kind=external_content and normally receives trust_class=external_untrusted, so it may be quarantined.
>
> Returns whether the raw episode was stored, reused, quarantined, or rejected; whether derived processing is queued or complete; and any policy/reconciliation guidance.

**Flat arguments:**

| Argument | Required | Notes |
|---|---|---|
| event_id | yes | Stable idempotency key supplied by the host |
| event_kind | yes | preference, constraint, decision, commitment, correction, tool_outcome, failure, checkpoint, task_outcome, or lesson_candidate |
| source_kind | yes | user, tool, repository, connector, agent, external_content, or host_lifecycle |
| source_id | yes | Stable source/trace identifier |
| content | yes | One self-contained event, not a transcript dump |
| t_ref | yes | When the event occurred or became observable |
| scope | yes | Most restrictive valid scope |
| project | no | Required by project adapters unless the mapping is deterministic |
| actor | no | User, agent, tool, connector, or concrete actor ID |
| trust_class | no | explicit_user, verified_tool, repository_or_internal_source, authoritative_connector, agent_inference, or external_untrusted; derived from source and connector policy by default, while explicit override requires policy authority |
| visibility_scope | no | Cannot be broader than scope |
| policy_tags | no | Security, retention, and privacy labels |
| session_id | no | Host session correlation |
| task_id | no | Cross-session task correlation |
| context_receipt_id | no | Links the event to prior memory exposure |
| outcome_status | no | success, partial, failure, cancelled, or unknown |

**Decision-ready return:**

- capture_status: stored, duplicate, quarantined, or rejected;
- episode_id;
- projection_status: queued, complete, partial, unsupported, or failed;
- durable_job_id when queued;
- policy decisions and redacted reasons;
- reconciliation summary when already available;
- guidance for user confirmation, retry, explain, or retraction.

**Idempotency rule:** The same event_id in the same scope/project/source identity must return the same episode or a conflict error. It must never silently create a second episode because the retry happened at a later wall-clock time.

### 7.4 Existing product tools: required corrections

| Tool | Role after the facade | Required correction |
|---|---|---|
| ingest | Raw-source ingestion, connectors, diagnostics | Describe the exact idempotency tuple; do not present it as the normal agent write path |
| extract | Explicit extraction/backfill and compatibility inline capture | Add missing project/source-policy metadata if inline mode remains public; dynamic guidance must surface partial projection/reconciliation state |
| resolve | Advanced resolve-or-create operation | State that it may add aliases or create entities; do not annotate as read-only |
| assemble_context | Advanced/raw retrieval and compatibility | Document view-mode/query semantics precisely; because it currently records access heat, either remove/move that side effect or do not mark it protocol-read-only |
| explain | Full provenance and relation evidence | Do not claim it proves causal use in an answer; return receipt/relation evidence where available |
| invalidate | Source-fact retraction | Replace “outdated or superseded” language with erroneous/withdrawn/corrupted/incorrectly-ingested source evidence |
| open_app/app_command | Human review and interactive administration | Keep outside the compact default agent profile |

### 7.5 Tool annotations

Annotations are untrusted hints, not authorization or enforcement.

| Tool | Intended annotations |
|---|---|
| memory_prepare_task | readOnlyHint=false while durable receipts/access telemetry are written; destructiveHint=false, idempotentHint=true with request_id, openWorldHint=false |
| memory_record_event | readOnlyHint=false, destructiveHint=false, idempotentHint=true, openWorldHint=false |
| explain | readOnlyHint=true, destructiveHint=false, openWorldHint=false if verification confirms no durable access-side effects |
| assemble_context | readOnlyHint=false while it records access heat; it may become readOnlyHint=true only after the side effect is removed or moved outside the operation contract |
| resolve | readOnlyHint=false, destructiveHint=false, idempotentHint=true only after retry behavior is tested |
| ingest | idempotentHint=true only when exact identity semantics are documented and tested |
| extract | Not read-only; task support remains useful for explicit long-running extraction |
| invalidate | readOnlyHint=false, destructiveHint=true, openWorldHint=false |
| app_command | Unannotated unless split by homogeneous action semantics |

The implementation must verify the actual rmcp 2.2.0 annotation API before changing macros/builders.

### 7.6 Intent coverage

| Agent intent | Default tool path | Calls |
|---|---|---|
| Start or resume a task with memory | memory_prepare_task | 1 |
| Recall before an answer or action | memory_prepare_task | 1 |
| Persist a decision/preference/outcome/checkpoint | memory_record_event | 1 |
| Retrieve full evidence for selected items | memory_prepare_task -> explain | 2 |
| Retract a bad source fact | invalidate | 1 |
| Perform interactive review/admin workflow | open_app -> app_command | 2 |

No ordinary discipline intent requires three sequential agent-controlled calls.

---

## 8. Lifecycle bridge architecture

### 8.1 Shared bridge core

The bridge core is a small policy and transport layer, not another memory database. It owns:

- host event normalization;
- project/scope/session/task resolution;
- trigger classification and risk policy;
- event salience and source-kind mapping;
- stable event IDs;
- retries and deadlines;
- degraded-mode behavior;
- context receipt propagation;
- redaction before capture;
- telemetry;
- transport selection.

The bridge calls the same capability service through:

1. a persistent MCP connection when the host exposes one to extensions;
2. a local MCP client sidecar where a persistent connection is practical;
3. the existing one-shot memory_mcp CLI as the command-hook fallback.

The CLI fallback is acceptable because it delegates to the same service/tool modules. It must not grow a second set of memory rules.

### 8.2 Client compatibility matrix as of 2026-07-18

| Client/harness | Reliable automatic points | Gaps | Required adapter |
|---|---|---|---|
| Claude Code | SessionStart, UserPromptSubmit, PreToolUse, PostToolUse, PreCompact, Stop/SessionEnd as supported by current hook contract | Shell/runtime portability and duplicate finalization | Full plugin: prompt recall, tool outcome capture, checkpoint, final flush |
| Codex CLI/app host | SessionStart, PreToolUse, PostToolUse, PermissionRequest in the current stable hooks feature | Current official docs say UserPromptSubmit and Stop are unsupported; answer-only turns cannot be fully enforced by hooks | Session orientation + pre/post-tool enforcement; model instructions and compaction prompt cover remaining gaps |
| Custom API/agent loop | Before every model call, before/after every tool, checkpoint, end | Integrator must adopt middleware | Reference middleware with conformance tests |
| OpenCode | Plugin/event capture is possible but must be verified against the installed release | Event names and payloads vary | Dedicated adapter after live contract verification |
| Gemini CLI | System instructions and MCP setup are established; hook lifecycle must be verified | Do not assume Claude-compatible events | Instructions/compaction profile first; hooks only after verification |
| VS Code/Cursor/Windsurf/other MCP hosts | Project instructions and MCP are broadly available; lifecycle varies | No universal pre-prompt or end event | Host-specific profile where supported; otherwise degraded prompt-assisted mode |
| Bare MCP | Model-selected tools only | No reliable initiation or automatic checkpoint | Server instructions, capability tools, and explicit degraded-mode documentation |

This matrix is versioned evidence, not a permanent truth. Each adapter release must record the host version and run a contract smoke test.

### 8.3 Event mapping

#### Claude Code

- SessionStart: orientation-only memory_prepare_task.
- UserPromptSubmit: focused memory_prepare_task using the actual prompt.
- PreToolUse: memory_prepare_task for configured significant/high-risk tools; avoid recalling before every harmless read.
- PostToolUse: memory_record_event with source_kind=tool, trust_class=verified_tool, and actual result status.
- PreCompact: synchronous checkpoint capture with bounded content.
- Stop/SessionEnd: task outcome/unresolved-state capture and durable flush; use once-only markers to avoid duplicate finalization.

#### Codex

- SessionStart startup/resume/clear: orientation pack.
- PreToolUse: focused recall for apply_patch, shell mutations, external actions, and selected MCP tools.
- PostToolUse: verified outcome capture.
- PermissionRequest: optional high-risk freshness check before approval.
- Compaction: use the supported compact-prompt mechanism to force a checkpoint call until a native hook exists.
- Answer-only turns: model instructions call memory_prepare_task; report this path as prompt-assisted, not deterministic.

#### Custom harness

- Before every model call, the controller decides whether the turn is memory-eligible and invokes memory_prepare_task before constructing the prompt.
- Before high-risk tools, it refreshes with risk_level=high.
- After every tool result, it captures verified outcomes according to write policy.
- At checkpoint/end, it persists state synchronously.

### 8.4 Trigger budget

“Always use memory” must not mean “call memory on every token or trivial read.”

The bridge uses event-based triggers:

- always at session/resume/checkpoint;
- before memory-dependent prompts;
- before significant or high-risk actions;
- after verified state-changing results;
- after explicit salient user statements;
- after decisions, corrections, commitments, failures, and task outcomes.

It skips:

- greetings and small talk;
- repeated calls with the same task/receipt and no state change;
- harmless file reads already covered by a fresh receipt;
- generated prose that contains no durable event.

---

## 9. Write policy and security

### 9.1 Automatic capture policy

| Event/source | Default action |
|---|---|
| Explicit user preference, constraint, correction, or commitment | Store with source_kind=user and trust_class=explicit_user |
| Decision with rationale confirmed by user or observed in an accepted artifact | Store |
| Verified tool result affecting project state | Store outcome; derive facts according to policy |
| Failure with reproducible root cause and evidence | Store |
| Task checkpoint/outcome/unresolved blocker | Store with session/task identity |
| Agent inference or hypothesis | Store only as agent_inference, or keep session-local; never present as established truth |
| Repository/document claim | Store with source and trust; do not reinterpret embedded instructions as user policy |
| External web/email/Slack/document content | Quarantine by default unless a trusted connector policy authorizes the claim type |
| Secret, credential, token, private key, session cookie | Reject/redact; never persist as normal memory |
| Small talk, transient phrasing, duplicated context | Do not store |
| Whole transcript | Do not promote wholesale; optional restricted archive uses a separate retention policy |

### 9.2 Trust classes

Recommended initial order:

1. authoritative_connector — explicitly configured for a claim schema/domain;
2. explicit_user — authoritative for the user’s own preferences and instructions, subject to security policy;
3. verified_tool — authoritative for the observed outcome of that tool, not for arbitrary instructions inside its output;
4. repository_or_internal_source — trusted only within its declared project/policy domain;
5. agent_inference — non-authoritative;
6. external_untrusted — quarantined for promotion and high-risk decisions.

Trust is scoped, not global. A build tool result can confirm that a test passed; it cannot establish the user’s authentication policy.

### 9.3 Poisoning controls

- Memory content is wrapped and labeled as evidence, never concatenated as privileged instructions.
- External content cannot request its own promotion to memory.
- Derived records inherit the least-trusted supporting source unless an explicit policy says otherwise.
- Compaction excludes or separately labels untrusted material.
- High-risk actions cannot rely solely on agent_inference or external_untrusted memory.
- Procedural promotion requires repeated verified success, explicit user confirmation, or authoritative documentation.
- Security evaluation includes explicit command insertion, policy-conformant fake facts, false precedents, salience/repetition attacks, and procedure poisoning.

### 9.4 Retraction and privacy

Four operations must remain distinct:

| Operation | Meaning | Public surface |
|---|---|---|
| Claim supersession | The world changed from a known time | Claim reconciliation |
| Claim correction | Earlier derived assertion was wrong for the same validity context | Claim reconciliation with explicit evidence |
| Source-fact retraction | Source was erroneous, withdrawn, corrupted, or incorrectly ingested | invalidate |
| Privacy purge/redaction | Data must no longer be retained or exposed | Administrative, separately authorized workflow |

The last operation may require destructive storage work and cannot be implemented by the append-only invalidate contract alone.

---

## 10. Background processing

### 10.1 Durable job state machine

Each accepted event creates a durable projection job atomically with the raw episode:

    queued -> leased -> running -> complete
                      -> partial
                      -> retry_wait -> queued
                      -> dead_letter
                      -> cancelled

Required properties:

- deterministic job and projection IDs;
- create-or-validate idempotency;
- expiring leases;
- bounded retries with reason codes;
- dead-letter inspection and replay;
- cancellation at safe boundaries;
- restart recovery;
- per-scope/project isolation;
- no raw sensitive content in metrics;
- lag and failure observability.

The generic durable-work mechanism planned for claim reconciliation should be reused where practical. MCP TaskStore must not be mistaken for this queue.

### 10.2 Projection stages

1. Validate source metadata and write policy again at worker boundary.
2. Extract entities/facts from the immutable episode.
3. Resolve entities under the correct namespace/project/policy.
4. Build versioned claim projections where deterministic schemas apply.
5. Reconcile claims using the accepted claim design.
6. Create/update embeddings and search indexes.
7. Link episodes, facts, entities, tasks, and outcomes.
8. Produce bounded summaries or lesson candidates where configured.
9. Invalidate affected retrieval caches only after committed relation/projection changes.
10. Persist status, fingerprints, and diagnostics.

### 10.3 Incremental consolidation

Adopt CocoIndex-style incremental principles:

- fingerprint raw input, extractor code/version, model/signature, and policy version;
- recompute only affected projections;
- treat vector indexes and summaries as rebuildable sidecars;
- preserve source lineage through every transformation;
- run backfills outside startup migrations;
- expose partial freshness rather than silently mixing incompatible projections.

### 10.4 Consolidation policy

- Raw episodes are immutable evidence.
- Duplicate derived records may be merged only through traceable relations.
- Coexisting facts remain coexisting when cardinality/qualifiers allow them.
- Reflection summaries never replace their supporting evidence.
- Procedural lessons remain candidates until promotion gates pass.
- Lifecycle decay may affect ranking/archival, but cannot decide real-world supersession.
- LLM-based reflection is optional and feature-gated; the default path remains local and deterministic where the project requires it.

---

## 11. Observability and audit

### 11.1 Required trace events

- bridge_event_received;
- memory_pre_read_started/completed/failed/degraded;
- context_receipt_created/injected;
- action_started/completed with receipt correlation;
- capture_candidate_classified;
- memory_event_stored/duplicate/quarantined/rejected;
- projection_job_queued/leased/retried/completed/dead_lettered;
- claim_relation_changed;
- correction_or_supersession_lag;
- cross_scope_access_denied;
- privacy_redaction_or_purge_audit;
- client_adapter_version and policy_version.

### 11.2 Metrics

| Metric | Meaning |
|---|---|
| Eligible pre-read coverage | Share of eligible prompt/action events with a receipt or explicit degraded event |
| Salient-event capture recall | Share of oracle-labeled durable events captured |
| Write precision | Share of stored/promoted events that should have been durable |
| Quarantine precision/recall | Correct handling of untrusted candidates |
| Retrieval recall and grounding | Relevant evidence returned and source-supported |
| Exposure-to-use rate | Memory exposed and later cited/applied; distinguish observed correlation from agent self-report |
| Action grounding accuracy | Correct tool and parameters derived from prior memory |
| Stale-memory influence | Actions/answers affected by obsolete or invalid evidence |
| Abstention accuracy | Correct insufficient-support behavior |
| Duplicate/coexisting-fact error rate | Overwrite/merge errors versus legitimate coexistence |
| Correction/supersession propagation lag | Time until current retrieval reflects a committed relation |
| Queue lag and dead-letter rate | Background health |
| Cross-scope denial/leak rate | Isolation correctness |
| Foreground latency and token overhead | End-to-end cost added by discipline |

### 11.3 What can and cannot be proven

- A context receipt proves that memory was retrieved and exposed by the bridge.
- A citation or used_fact_ids field is an agent claim about use.
- Parameter matching and controlled ablations provide stronger evidence that memory affected an action.
- explain provides provenance for evidence, not proof of model causality.

---

## 12. Evaluation strategy

### 12.1 Experimental modes

Every discipline evaluation compares:

1. no external memory;
2. long-context only;
3. bare MCP, model-selected calls;
4. server instructions and project rules only;
5. host-enforced lifecycle bridge;
6. host-enforced bridge plus durable background processing.

This isolates the effect of the enforcement layer from storage/retrieval quality.

### 12.2 External benchmark families

- LongMemEval: extraction, multi-session, temporal, updates, abstention.
- MemoryAgentBench: retrieval, learning, long-range understanding, selective forgetting.
- MemFail: summary, storage, coexisting-fact, and retrieval failures.
- Mem2ActBench-style tasks: tool choice and parameter grounding.
- MemoryArena-style tasks: interdependent cross-session action loops.
- Memora/FAMA: obsolete-memory influence.
- MPBench-style poisoning cases: write-channel and persistent influence attacks.

External benchmark claims must report the exact dataset version, split, reader model, retrieval budget, and metric. Vendor numbers are not release gates until reproduced.

### 12.3 Internal coding-agent corpus

Build real/replayable sessions covering:

- an earlier architecture decision that should constrain a later edit;
- a user correction that must override an agent assumption without retracting valid source evidence;
- a failed approach that must not be repeated;
- a verified successful fix reused in a later task;
- a task resumed after compaction;
- a project-name ambiguity;
- a cross-project or cross-scope near-match;
- coexisting preferences/constraints;
- conflicting sources without enough evidence for supersession;
- a source correction versus a real-world transition;
- an external document containing memory-write instructions;
- a false successful precedent;
- a poisoned procedure candidate;
- memory service outage and degraded behavior;
- duplicate hook delivery and process restart.

### 12.4 Baseline and release gates

Before implementation, capture a baseline on the current tools and at least three host modes.

Hard deterministic gates:

- every configured eligible lifecycle event produces either a context receipt/capture result or an explicit degraded event;
- duplicate event delivery never creates a second raw episode;
- no cross-scope/project/policy leakage;
- external_untrusted content is never automatically promoted to user preference, security policy, or procedure;
- contradictions do not retract source facts;
- source-fact retraction does not masquerade as claim supersession;
- queued work survives restart and is either completed or visible in dead letter;
- current public schemas remain backward compatible unless a versioned migration is approved.

Empirical quality targets are set only after the baseline corpus is labeled. The first release gate should require statistically meaningful improvement over bare MCP and instructions-only modes without unacceptable write-precision, poisoning, latency, or token regressions.

---

## 13. Implementation plan

The work packages below are ordered by dependency. They describe implementation; they do not implement it.

### WP0 — Freeze the discipline contract and baseline

**Outcome:** A versioned, testable definition of when memory must be read or written.

**Work:**

1. Create a canonical discipline contract under docs/agent_integration describing triggers, event kinds, source/trust classes, degraded behavior, and host mappings.
2. Record the current MCP/CLI behavior of all six memory primitives and two app tools.
3. Correct documentation conflicts with ADR-0002 through ADR-0015.
4. Build the first oracle-labeled discipline corpus from real or replayed coding-agent sessions.
5. Run baseline modes: no memory, bare MCP, instructions only, and any existing manual workflow.
6. Record current Claude Code and Codex hook contract fixtures with host versions.

**Likely areas:**

- docs/agent_integration/
- docs/AGENT_MEMORY_DISCIPLINE_PLAN.md
- tests/fixtures/evals/discipline/
- tests/eval_discipline.rs
- host-contract fixture files

**Acceptance:**

- Each trigger has an unambiguous eligible/non-eligible example.
- Each event kind has source, trust, scope, and redaction rules.
- Baseline report includes write precision as well as recall coverage.
- Accepted claim/retraction terminology is used consistently.

### WP1 — Correct the existing public contracts

**Outcome:** Existing tools no longer teach agents incorrect mutation or temporal semantics.

**Work:**

1. Rewrite invalidate descriptions/guidance as source-fact retraction only.
2. Mark resolve as mutating in descriptions and annotations.
3. Document exact ingest identity/idempotency behavior.
4. Correct extract inline-mode metadata and retry semantics, or explicitly limit it to compatibility use.
5. Make extraction guidance conditional on projection/reconciliation outcomes.
6. Strengthen concise SERVER_INSTRUCTIONS with the read-before-act and write-after-salience contract plus degraded-mode honesty.
7. Add accurate annotations after verifying rmcp support.
8. Add a read-only discipline/integration resource for discovery; do not rely on it for automatic injection.

**Likely areas:**

- src/mcp/handlers.rs
- src/tools/params.rs
- src/tools/ingest.rs
- src/tools/extract.rs
- src/tools/resolve.rs
- src/tools/invalidate.rs
- src/mcp/resources.rs
- schema/description conformance tests

**Acceptance:**

- No tool is incorrectly labeled read-only or idempotent.
- No public text tells an agent to invalidate a source fact for routine supersession.
- Existing clients remain schema-compatible.
- Tool descriptions, server instructions, and canonical contract pass semantic conformance tests.

### WP2 — Add source-aware event capture foundation

**Outcome:** A significant event can be recorded once with stable identity and complete policy metadata.

**Work:**

1. Add explicit event identity, event kind, source kind, actor, trust, session/task correlation, and policy fields to the service model.
2. Define validation and precedence: explicit project, session mapping, repository config, or an actionable ambiguity error.
3. Implement create-or-validate idempotency for event_id.
4. Add write-policy classification outcomes: accept, quarantine, reject, redact.
5. Atomically persist the raw episode and durable projection job.
6. Ensure background work inherits scope/project/visibility/policy/trust.
7. Add one-shot CLI support for the capability call so command hooks can use the same service path.

**Likely areas:**

- src/models/request.rs and related domain/access/provenance modules
- src/service/ingestion.rs
- new focused service module for event capture policy
- storage migrations and indexes
- src/cli/args.rs and src/cli/commands/
- integration tests for duplicate delivery, ambiguity, quarantine, and restart

**Acceptance:**

- Retry at a later time returns the original event result.
- Same event ID with conflicting immutable identity fails loudly.
- No policy field is lost between foreground capture and background job.
- Secrets and untrusted self-promotion cases are rejected/quarantined.

### WP3 — Implement the two capability tools

**Outcome:** Common discipline paths take one agent-facing call.

**Work:**

1. Implement memory_prepare_task over existing context retrieval, recent state, claim relation enrichment, and minimal provenance.
2. Return context_receipt_id and persist receipt metadata.
3. Implement memory_record_event over WP2 capture.
4. Add decision-ready partial/degraded responses and guidance.
5. Register a compact default agent tool profile while preserving the full compatibility profile.
6. Add MCP and CLI end-to-end contract tests.
7. Update the memory-mcp skill and README only after the schemas are final.

**Likely areas:**

- new src/tools/prepare_task.rs
- new src/tools/record_event.rs
- src/tools/params.rs
- src/mcp/handlers.rs
- src/service/context/
- new receipt model/store module
- src/cli/
- tests/tools_e2e.rs and integration tests

**Acceptance:**

- Start/resume recall is one call and includes current facts, unresolved relations, provenance handles, and insufficient-support state.
- Significant capture is one idempotent call and never requires agent-controlled ingest -> extract.
- The default agent profile stays within a small tool budget.
- Full product tools remain available for compatibility and debugging.

### WP4 — Build the shared lifecycle bridge

**Outcome:** Host events invoke the capability tools independently of model attention.

**Work:**

1. Implement a host-neutral bridge core for policy, identities, transport, retry, receipts, and telemetry.
2. Add a CLI transport using capability subcommands; add persistent MCP transport where the host permits it.
3. Implement the Claude Code adapter and once-only finalization.
4. Implement the Codex adapter using only currently supported events; use model instructions/compact prompt for unsupported boundaries.
5. Publish a custom-harness middleware reference.
6. Add host-versioned contract fixtures and smoke tests.
7. Document bare-MCP and prompt-assisted degradation explicitly.

**Likely areas:**

- docs/agent_integration/bridge/
- packaged client adapters/plugins
- examples or a small integration crate only if justified by reuse
- host contract tests and fixtures

**Acceptance:**

- Hooks never depend on the model voluntarily choosing a memory tool.
- Claude pre-prompt recall and tool outcome capture are deterministic in integration tests.
- Codex pre/post-tool enforcement is deterministic; unsupported answer-only coverage is reported honestly.
- Duplicate session-end/stop signals do not create duplicate captures.
- Memory outage produces the configured degraded/fail-closed behavior.

### WP5 — Durable background projection and consolidation

**Outcome:** Captured events become queryable derived memory without blocking the interaction or losing failures.

**Work:**

1. Reuse or generalize durable-job mechanics from claim reconciliation.
2. Add leases, retries, dead letter, replay, cancellation, and restart recovery.
3. Move extraction, embedding, linking, and claim projection behind the durable job.
4. Add input/code/model/policy fingerprints and incremental recomputation.
5. Add bounded consolidation for summaries and lesson candidates.
6. Gate procedural promotion on evidence.
7. Surface queue lag and partial freshness in memory_prepare_task and operator views.
8. Keep MCP Tasks as an optional status interface, not the job owner.

**Likely areas:**

- src/service/jobs/ or a shared durable-work module
- src/service/episode/
- src/service/claims/ dependency boundary
- src/service/embedding/
- src/service/lifecycle/
- storage migrations
- lifecycle/worker integration tests

**Acceptance:**

- A process restart cannot lose an accepted event or hide a failed projection.
- Reprocessing the same fingerprint is idempotent.
- One failed stage does not corrupt the raw episode or unrelated projections.
- Current retrieval reports incomplete/stale derived state rather than silently pretending it is fresh.

### WP6 — Security, privacy, and audit controls

**Outcome:** Automatic memory does not silently turn external content or model guesses into trusted long-term policy.

**Work:**

1. Enforce trust inheritance across facts, claims, summaries, and lessons.
2. Add quarantine review and release workflows through an admin/app surface.
3. Add high-risk retrieval/action policy.
4. Add context receipt correlation and exposure/action audit views.
5. Add separately authorized privacy export, redaction, and purge design/implementation.
6. Add poisoning and cross-scope adversarial suites.

**Likely areas:**

- access/provenance models
- capture policy service
- context assembly
- MCP Apps/admin CLI
- audit storage and explain enrichment
- security test fixtures

**Acceptance:**

- Retrieved low-trust content cannot become privileged instructions.
- Cross-scope and counterpart-relation enrichment cannot leak existence or content.
- Privacy purge is not implemented as ordinary invalidate.
- Audit distinguishes exposed, agent-claimed-used, and action-grounded evidence.

### WP7 — Evaluation, observability, and staged rollout

**Outcome:** Default-on discipline is enabled only with evidence that it improves action quality safely.

**Work:**

1. Complete discipline, action-grounding, stale-influence, and poisoning metrics.
2. Add Prometheus/structured telemetry with bounded label cardinality.
3. Reproduce selected external benchmarks with pinned versions.
4. Compare all experimental modes from section 12.1.
5. Roll out in observe-only, shadow, opt-in enforced, and default-on stages.
6. Publish a release evidence report with client versions, corpus version, confusion matrices, latency/token cost, and known gaps.

**Likely areas:**

- tests/eval_discipline.rs
- tests/eval_action_grounding.rs
- tests/eval_memory_poisoning.rs
- docs/evals/AGENT_MEMORY_DISCIPLINE.md
- observability modules and dashboards

**Acceptance:**

- Hard deterministic gates in section 12.4 pass.
- Host-enforced mode improves action grounding over bare MCP and instructions-only baselines.
- Write precision and poisoning resistance do not regress beyond approved gates.
- The release report states which client boundaries remain probabilistic.

### 13.1 Dependency order

    WP0
      -> WP1
      -> WP2
      -> WP3
      -> WP4
      -> WP5
      -> WP6
      -> WP7/default-on

WP0 evaluation work continues throughout. WP6 threat modeling begins during WP2 even though the full operator controls land later. Claim reconciliation is a dependency for complete relation-aware retrieval, but the bridge and raw event capture can be developed against a backward-compatible partial state.

---

## 14. Rollout modes

| Mode | Behavior | Purpose |
|---|---|---|
| Observe-only | Classify eligible reads/writes; do not call or persist | Validate trigger policy without side effects |
| Shadow | Call retrieval/capture policy in parallel; do not inject or promote | Measure quality, latency, and false positives |
| Opt-in enforced | Inject receipts and persist accepted events for selected users/projects | Real-world validation |
| Default-on per adapter | Enforcement enabled only on hosts that passed contract/security gates | Production |
| Bare MCP degraded | Capability tools available but initiation remains model-controlled | Compatibility |

Rollback disables bridge enforcement and projection promotion without deleting raw evidence, jobs, claims, or audit history.

---

## 15. Explicit non-goals

- Making the MCP server a complete agent runtime.
- Claiming that server instructions or AGENTS.md guarantee tool invocation.
- Using a specialized Memory Agent as the required lifecycle controller.
- Storing every message as a trusted fact.
- Replacing raw evidence with summaries.
- Letting an LLM be the sole authority for freshness, correction, or trust.
- Adding dozens of agent-facing CRUD/search tools.
- Reimplementing the accepted claim-reconciliation subsystem.
- Treating MCP Tasks as a durable scheduler.
- Treating invalidate as privacy deletion.
- Enabling automatic procedural self-improvement without promotion evidence.
- Declaring vendor benchmark numbers as local release evidence.

---

## 16. Definition of done

The mechanism is complete when:

1. Supported host adapters invoke memory at their documented lifecycle boundaries without model choice.
2. The common read and write workflows each require one agent-facing capability call.
3. Every accepted event is durably stored once with source, trust, scope, project, time, and provenance.
4. Derived work survives restart and exposes failures.
5. Claim contradiction, supersession, correction, source retraction, and privacy purge remain distinct.
6. Retrieved memory is source-labeled data and cannot silently become privileged instructions.
7. Every pre-action memory exposure has a context receipt correlated with later action telemetry.
8. Evaluation demonstrates improved memory-grounded action, not only improved QA retrieval.
9. Stale influence, false-positive writes, poisoning, latency, and token cost remain within evidence-backed release gates.
10. Bare MCP and unsupported host boundaries are labeled as degraded/probabilistic.

---

## 17. Primary and official references

### Research

- [Generative Agents](https://arxiv.org/abs/2304.03442)
- [MemGPT](https://arxiv.org/abs/2310.08560)
- [LongMemEval](https://arxiv.org/abs/2410.10813)
- [A-MEM](https://arxiv.org/abs/2502.12110)
- [MemoryAgentBench](https://arxiv.org/abs/2507.05257)
- [MemFail](https://arxiv.org/abs/2605.26667)
- [MemoryArena](https://arxiv.org/abs/2602.16313)
- [EvoMemBench](https://arxiv.org/abs/2605.18421)
- [Mem2ActBench](https://aclanthology.org/2026.acl-long.370/)
- [LightMem](https://aclanthology.org/2026.acl-long.588/)
- [AgeMem](https://aclanthology.org/2026.acl-long.981/)
- [Memora / FAMA](https://aclanthology.org/2026.findings-acl.1337/)
- [LongMemEval-V2](https://arxiv.org/abs/2605.12493)
- [Memory poisoning / MPBench](https://arxiv.org/abs/2606.04329)
- [Zep / Graphiti paper](https://arxiv.org/abs/2501.13956)

### Protocol and host behavior

- [MCP server primitives](https://modelcontextprotocol.io/specification/2025-11-25/server)
- [MCP server instructions](https://blog.modelcontextprotocol.io/posts/2025-11-03-using-server-instructions)
- [MCP tool annotations](https://blog.modelcontextprotocol.io/posts/2026-03-16-tool-annotations)
- [Claude Code hooks reference](https://docs.anthropic.com/en/docs/claude-code/hooks)
- [Claude Code hooks guide](https://docs.anthropic.com/en/docs/claude-code/hooks-guide)
- [Codex hooks](https://developers.openai.com/codex/hooks)
- [Codex advanced configuration](https://developers.openai.com/codex/config-advanced)

### Compared projects

- [Mem0](https://github.com/mem0ai/mem0)
- [Cognee](https://github.com/topoteretes/cognee)
- [Engram](https://github.com/Gentleman-Programming/engram)
- [CocoIndex](https://github.com/cocoindex-io/cocoindex)
- [MentisDB](https://github.com/CloudLLM-ai/mentisdb)
- [MenteDB](https://github.com/nambok/mentedb)
- [MinnsDB](https://github.com/Minns-ai/MinnsDB)
- [MemPalace](https://github.com/mempalace/mempalace)
- [Graphiti](https://github.com/getzep/graphiti)
- [Letta](https://github.com/letta-ai/letta)
- [LangMem](https://github.com/langchain-ai/langmem)
- [Hindsight](https://github.com/vectorize-io/hindsight)

### Internal authority

- docs/MEMORY_SYSTEM_SPEC.md
- docs/CONTRADICTION_DETECTION_DESIGN.md
- docs/adr/0002-contradiction-does-not-invalidate-facts.md
- docs/adr/0008-require-source-continuity-for-automatic-supersession.md
- docs/adr/0009-separate-claim-supersession-from-fact-retraction.md
- docs/adr/0015-distinguish-correction-from-supersession.md
- docs/superpowers/plans/2026-07-18-claim-reconciliation.md
- docs/INTENT_DRIVEN_MCP_DESIGN_GUIDE.md
- docs/LIFECYCLE_BACKGROUND_JOBS.md
