---
name: memory-mcp
description: "Durable-facts layer for AI agents. Use when persisting a verified source as an episode, extracting entities and facts, resolving entity aliases, ranking facts for a query, citing retrieved facts, or invalidating an outdated fact. Triggers: ingest, extract, resolve, assemble_context, explain, invalidate, durable fact, knowledge graph, bi-temporal, source_id, t_ref, episode_id, fact_id, source_id conflict, scope denied."
compatibility: Requires the `memory_mcp` server (MCP stdio) or its CLI binary (`memory_mcp <subcommand>`). The eight tools and their semantics are described in this skill; do not invent tools, parameters, or behavior outside of what is documented here, in the server's tool schema, or in `references/`. Never store secrets, tokens, or credentials.
---

# Memory MCP

The durable-facts layer for AI agents working with the `memory_mcp` server (Rust, SurrealDB, optional candle embeddings). It holds verified sources as **episodes**, the structured knowledge extracted from them as **facts**, and the entity graph that links them. The server exposes one tool contract on two surfaces — MCP stdio and a CLI subcommand — that share the same parameters, semantics, and SOPs.

This skill is the **direct contract** for single-server / single-batch operations on the `memory_mcp` tool surface. For multi-source synthesis, stakeholder briefs, or session-spanning decision logs, hand off to whatever synthesis workflow the host agent provides; the memory layer is the same, the scope is larger.

## What the server is

The `memory_mcp` server is a Rust binary with three run modes and eight tools (six canonical + two opt-in).

**Run modes:**

| Mode | Command | Purpose |
|---|---|---|
| MCP stdio (default) | `memory_mcp serve` (or `cargo run -- serve`) | The agent runtime starts the server and calls its tools over MCP. |
| File-system watch | `memory_mcp watch --path ./data` (or `--features cli-watch -- serve --watch ./data`) | Auto-ingest files as they appear. Operational mode, not a tool surface. |
| Re-embed | `memory_mcp reembed` | Maintenance — rebuilds fact embeddings after a model change. Operational mode, not a tool surface. |

**Tool surfaces** — same contract on both transports:

| Tool | MCP name | CLI subcommand | Required from caller | Returns |
|---|---|---|---|---|
| Persist a verified source | `mcp_memory-mcp_ingest` | `memory_mcp ingest` | `source_type`, `source_id`, `content`, `t_ref`, `scope` | `episode_id` |
| Derive facts | `mcp_memory-mcp_extract` | `memory_mcp extract` | `episode_id` (or inline `content`) | `entities`, `facts`, `links` |
| Canonicalize a name | `mcp_memory-mcp_resolve` | `memory_mcp resolve` | `entity_type`, `canonical_name` | `entity_id` |
| Rank facts for a query | `mcp_memory-mcp_assemble_context` | `memory_mcp assemble-context` | `query`, `scope` | ranked context items |
| Cite a retrieved fact | `mcp_memory-mcp_explain` | `memory_mcp explain` | `context_items` | ready-to-cite quotes |
| Mark a fact outdated | `mcp_memory-mcp_invalidate` | `memory_mcp invalidate` | `fact_id`, `reason`, `t_invalid` | confirmation |

**Opt-in app tools** (require the `mcp-apps` cargo feature; no CLI equivalent):

| Tool | Purpose |
|---|---|
| `mcp_memory-mcp_open_app` | Open a session-backed app view (`inspector`, `diff`, `ingestion_review`, `lifecycle`, `graph`). Read-only entry point; `app_command` drives the session. |
| `mcp_memory-mcp_app_command` | Execute coarse-grained actions inside an open app session. Use only when a canonical tool does not match the intent. |

CLI flag mapping, exact response shapes, and per-tool error patterns live in [source conventions](references/source-conventions.md) and [error recovery](references/error-recovery.md). Scope rules — the hard contract — live in [scope guide](references/scope-guide.md).

## Core principles

Four principles bind every SOP, guardrail, and tool call. They restate here once; the rules that depend on them live in the references.

1. **_Verified_ before claimed.** A capture is not "done" when `ingest` returns an `episode_id`; it is done when `extract` returns a fact set the caller has inspected. Empty `facts: []` and a failed call are both *states* — never collapse them into "captured".
2. **_Narrowest_ suitable scope.** The `scope` field is an access-policy contract, not a label for the audience. Pick the narrowest scope the content's policy permits; widen only with verified evidence that the broader scope is required. (See [scope guide](references/scope-guide.md).)
3. **Bi-temporal truth.** Every fact carries `t_ref` (valid time — when the fact was true) and `t_ingested` (transaction time — when the server recorded it). The caller sets `t_ref` truthfully; the server sets `t_ingested`. `assemble_context` honors both via `as_of`.
4. **Never overwrite, never delete.** A re-ingest under an existing `source_id` with different `content` is a **conflict**, not an update. To supersede a fact, `invalidate` the old fact with `t_invalid` and a reason; capture the new source under a new `source_id`. (See [source conventions](references/source-conventions.md#source_id-patterns).)

## SOP: memory capture

Use when a verified source — a confirmed message, a committed decision, a stable metric, a reconciled work item — must outlive the current session. Three steps. Capture is **not** complete until `extract` returns verified facts or the empty set is explicitly justified.

1. **Ingest.** Persist the source with a deterministic `source_id`, a real `t_ref` (the source's own timestamp, not now), the narrowest suitable scope, and bounded `content` (see [content rules](references/source-conventions.md#field-contract)). The `source_id` must round-trip: idempotent on same id+content, **conflict** on same id+different content (see principle #4 — never overwrite).

   *Completion:* `episode_id` returned; `scope` is the narrowest suitable; `content` is bounded, source-linked, and secret-free.

2. **Extract.** Run the extract tool against the episode. Inspect every returned fact and every warning. An empty `facts: []` means the episode is recorded but **no durable fact was captured** — a different result from "capture failed"; both must be reported.

   *Completion:* facts list inspected; each verified fact is named, or the empty set is justified against the source.

3. **Invalidate (only when superseding).** When a newer verified fact contradicts an active one, mark the old fact with the invalidate tool, a reason, and a `t_invalid`. (See principle #4 — never overwrite, never delete.)

   *Completion:* superseded `fact_id` carries `t_invalid` and a human-readable reason; the new fact is the only active one for the same claim.

A capture result is exactly one of: **verified** (episode + facts), **episode-only** (episode, no facts), or **pending** (operation failed; source handles preserved). Never report "captured" without naming which one.

## SOP: memory retrieval

Use when a current question, plan, or recommendation needs facts the agent does not already hold. Three steps. Retrieval is **not** complete until citations exist or the absence is explicit.

1. **Resolve (when entities matter).** When the query names a person, company, technology, or event, run the resolve tool for each ambiguous name to obtain a canonical `entity_id`. Skipping this step turns later fact linking into name-matching and silently misses facts recorded under an alias.

   *Completion:* every named entity has a canonical id, or the skip is justified because no fact will be linked to it.

2. **Assemble.** Run the assemble_context tool with the query, the narrowest suitable scope that can hold the answer, and an explicit `as_of` when point-in-time matters. Inspect every returned item: `confidence`, `rationale`, and `quote` are evidence, not decoration.

   *Completion:* every returned item has been read; verified facts are separated from inferences; the absence of an item is treated as "no durable fact", not "no truth".

3. **Explain (when citing).** Before quoting a fact in a user-facing artifact, pass the context item to the explain tool to get the source quote. A fact without provenance is a claim, not evidence.

   *Completion:* every cited fact carries a source quote and an `episode_id`; the user-facing text does not assert anything stronger than the quote supports.

Retrieval never modifies state. If the question reveals that a fact should be captured, route to the capture SOP — do not smuggle writes into a read path.

## SOP: opt-in apps (when `mcp-apps` is enabled)

`open_app` / `app_command` exist for **session-backed, multi-step work** that the canonical six tools cannot express as a single call. Use them only when one canonical tool cannot match the intent — interactive inspection, temporal diff, ingestion review, lifecycle maintenance, graph traversal.

1. **Open a session.** Call `mcp_memory-mcp_open_app` with the smallest `app` that matches the intent: `inspector` for an entity/fact/episode view, `diff` for two `as_of` snapshots, `ingestion_review` for a draft episode, `lifecycle` for scope-wide maintenance, `graph` for path between two entities. Required fields vary by app; everything else stays unset.

   *Completion:* `session_id` returned; `resource_uri` read at least once for the current view.

2. **Drive the session.** Call `mcp_memory-mcp_app_command` with the session id, a documented `action` (e.g. `approve_items`, `export_diff`, `expand_neighbors`, `close_session`), and only the parameters that action requires. Do not invent actions; the set is closed and documented in the tool description.

   *Completion:* each action returns whether the caller should re-read the app resource; the session reaches its terminal state (`close_session` or scope-driven expiry) without stray writes.

3. **Prefer canonical tools.** If the same business intent can be satisfied with one of the six canonical tools, use that — apps are a fallback, not a default. A session that "happens to" use `open_app` for a single-step task is a routing error.

## Guardrails

These are the leading-word rules from Core principles, phrased as hard guardrails. The detailed mechanics live in the linked references.

- **_Verified_**. Capture is not done on `ingest` alone. `extract` must return inspected facts, an empty set, or a documented failure — name which.
- **_Narrowest_ scope**. Match the access policy of the content, not the audience. Widen only with verified evidence. ([scope guide](references/scope-guide.md))
- **No secrets**. The `content` field holds verified source material. Tokens, credentials, PII, logs, and drafts do not belong there. ([content rules](references/source-conventions.md#field-contract))
- **No silent overwrite**. Same `source_id`, different content = conflict. `invalidate` the old fact, ingest under a new id. ([source_id patterns](references/source-conventions.md#source_id-patterns))
- **No writes in a read path**. Retrieval — `resolve` / `assemble_context` / `explain` — never calls `ingest`, `extract`, or `invalidate`.
- **No fabricated contracts**. Eight tools in total (six canonical + two opt-in). If a tool name or parameter is not documented here, in the server's schema, or in `references/`, it does not exist.

## SOP exit gate

The memory operation is complete when, for the chosen branch, every step reports its `Completion:` and the final result is one of:

- **verified** — capture with episode and inspected facts.
- **episode-only** — capture with an empty `facts: []` and the empty set justified.
- **pending** — capture that did not complete; `source_id` and any partial `episode_id` preserved.
- **assembled and cited** — retrieval with provenance for every quote.
- **session closed** — opt-in app workflow reached a terminal state with no stray writes.

Anything weaker is not done.
