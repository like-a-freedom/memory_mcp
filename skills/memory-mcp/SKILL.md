---
name: memory-mcp
description: "Memory MCP capture and retrieval: use when persisting verified sources as episodes, extracting facts, resolving entity aliases, assembling context for a query, or invalidating outdated facts. Triggers: ingest, extract, resolve, assemble context, explain provenance, invalidate fact, memory capture, durable fact, knowledge graph, bi-temporal."
compatibility: Requires configured `memory-mcp` MCP server. Operations are MCP-only; no CLI equivalent. Never store secrets, tokens, or credentials; never invent a Memory Agent invocation contract.
---

# Memory MCP

Memory MCP is the durable-facts layer. It holds verified episodes, the facts extracted from them, and the entity graph that links them. This skill is the **direct** contract: it fires when one MCP call (or a small bounded batch) is the right scope. For multi-source synthesis, stakeholder briefs, or decision logs, hand off to a Memory Agent — the memory layer is the same, the work is larger.

Two SOPs, one MCP surface. Each operation either **writes** (capture, including invalidation) or **reads** (retrieval). Mixing the two is a routing error.

## Compatibility

- MCP server: `memory-mcp` configured in the agent runtime. No CLI binary; no fallback path.
- Auth lives in MCP configuration. Commands, logs, and responses contain no tokens, PATs, or credentials.
- Bi-temporal: every fact carries `t_ref` (valid time, when the fact was true) and `t_ingested` (set by the server). `assemble_context` honors both.
- Scope is a hard contract: pick the narrowest suitable. See [scope guide](references/scope-guide.md).

## MCP contract

All six operations use canonical `server/tool` names. The agent supplies `t_ref`; the server sets `t_ingested`. Other optional fields are server-managed.

| Operation | Tool | Required from agent | Returns |
|---|---|---|---|
| Persist raw source | `memory-mcp/ingest` | `source_type`, `source_id`, `content`, `t_ref`, `scope` | `episode_id` |
| Derive facts | `memory-mcp/extract` | `episode_id` (or inline `content`) | `entities`, `facts`, `links` |
| Canonicalize a name | `memory-mcp/resolve` | `entity_type`, `canonical_name` | `entity_id` |
| Rank facts for a query | `memory-mcp/assemble_context` | `query`, `scope` | ranked context items |
| Cite a retrieved fact | `memory-mcp/explain` | `context_items` | ready-to-cite quotes |
| Mark a fact outdated | `memory-mcp/invalidate` | `fact_id`, `reason`, `t_invalid` | confirmation |

Optional fields and exact response shapes are in [source conventions](references/source-conventions.md) and [error recovery](references/error-recovery.md).

## SOP: memory capture

Use when a verified source — a confirmed message, a committed decision, a stable metric, a reconciled work item — must outlive the current session. Three steps. Capture is **not** complete until `extract` returns verified facts or the empty set is explicitly justified.

1. **Ingest.** Persist the source. Use a deterministic `source_id`, real `t_ref` (the source's own timestamp, not now), the narrowest suitable scope, and bounded `content` — never raw tool output, never secrets, never speculative drafts. The `source_id` must round-trip: a second ingest with the same id and content is idempotent; a second ingest with the same id and different content is a conflict, not an update.
   *Completion:* `episode_id` returned, `scope` is the narrowest suitable, `content` is bounded and source-linked.

2. **Extract.** Run `memory-mcp/extract` against the episode. Inspect every returned fact and every warning. An empty `facts: []` means the episode is recorded but **no durable fact was captured** — that is a different result from "capture failed", and both must be reported.
   *Completion:* facts list inspected; each verified fact is named, or the empty set is justified against the source.

3. **Invalidate (only when superseding).** When a newer verified fact contradicts an active one, mark the old fact with `memory-mcp/invalidate` and a reason. Never overwrite, never delete, never silently re-ingest with the same `source_id` and different content.
   *Completion:* superseded `fact_id` carries `t_invalid` and a human-readable reason; the new fact is the only active one for the same claim.

A capture result is either **verified** (episode + facts), **episode-only** (episode, no facts), or **pending** (operation failed; source handles preserved). Never report "captured" without one of these three.

## SOP: memory retrieval

Use when a current question, plan, or recommendation needs facts the agent does not already hold. Three steps. Retrieval is **not** complete until citations exist or the absence is explicit.

1. **Resolve (when entities matter).** When the query names a person, company, technology, or event, run `memory-mcp/resolve` for each ambiguous name to obtain a canonical `entity_id`. Skipping this step turns later fact linking into name-matching and silently misses facts recorded under an alias.
   *Completion:* every named entity has a canonical id, or the skip is justified because no fact will be linked to it.

2. **Assemble.** Run `memory-mcp/assemble_context` with the query, the narrowest suitable scope that can hold the answer, and an explicit `as_of` when point-in-time matters. Inspect every returned item: `confidence`, `rationale`, and `quote` are evidence, not decoration.
   *Completion:* every returned item has been read; verified facts are separated from inferences; the absence of an item is treated as "no durable fact", not "no truth".

3. **Explain (when citing).** Before quoting a fact in a user-facing artifact, pass the context item to `memory-mcp/explain` to get the source quote. A fact without provenance is a claim, not evidence.
   *Completion:* every cited fact carries a source quote and an `episode_id`; the user-facing text does not assert anything stronger than the quote supports.

Retrieval never modifies state. If the question reveals that a fact should be captured, route to the capture SOP — do not smuggle writes into a read path.

## Guardrails

- **Verified before claimed.** Never report capture success on `ingest` alone. `extract` must return verified facts, an explicitly empty set, or a documented failure.
- **Narrowest suitable scope.** `private-domain` for restricted content (HR, security, customer PII); `personal` for individual notes; `team` for shared work; `org` for company-wide. When in doubt, narrow and widen only with verified evidence that broader scope is required.
- **No secrets, no drafts, no raw tool output.** The `content` field holds verified source material. Tokens, credentials, customer PII, transient logs, and unverified drafts do not.
- **No silent overwrite.** A re-ingest with a different content under the same `source_id` is a conflict, not an update. Use `invalidate` for the old fact and ingest the new source under a new `source_id`.
- **No writes in a read path.** Retrieval never calls `ingest`, `extract`, or `invalidate`. If a fact needs recording, end the retrieval, then run the capture SOP.
- **No invented Memory Agent contract.** This skill uses direct MCP. A "Memory Agent" is a separate subagent pattern; its invocation contract is not defined here.

## SOP exit gate

The memory operation is complete when, for the chosen branch, every step reports its `Completion:` and the final result is one of: **verified** (capture), **episode-only** (capture, with empty `facts` justified), **pending** (capture, with source handles preserved), or **assembled and cited** (retrieval, with provenance for every quote). Anything weaker is not done.
