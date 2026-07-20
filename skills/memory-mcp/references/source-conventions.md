# Source Conventions

Detail for the memory capture SOP. The rule that matters lives in the parent skill; this file is the field catalog and the per-field rules.

## Field contract

The caller supplies these fields. Everything else is server-managed.

| Field | Type | Rule |
|---|---|---|
| `source_type` | enum | One of `email`, `document`, `conversation`, `ad-hoc`. Match the source, not the channel. |
| `source_id` | string | Deterministic, globally unique, round-trippable. See patterns below. |
| `content` | string | Bounded, source-linked, verified. Not raw tool output, not a draft, not a secret. |
| `t_ref` | ISO 8601 | The source's own timestamp in UTC (`...Z`). Not the moment of capture. |
| `scope` | enum | The narrowest suitable. See [scope guide](scope-guide.md). |
| `project` | string, optional | Set when the caller knows the project. Omit when it does not. |
| `t_ingested` | ISO 8601, optional | Override the server's transaction time only when the caller has a real reason. Default: now, set by the server. |
| `visibility_scope` | string, optional | Server-managed metadata. If a project requires a specific value, that project's skill defines it. |
| `policy_tags` | array, optional | Server-managed governance tags. Same rule: project skill defines if needed. |

`content` is the single most failure-prone field. Hold it to three rules — **bounded** (excerpt with location, not a 40-page document), **source-linked** (first sentence or metadata field carries provenance), **verified** (no draft, no paraphrase, no unconfirmed tool response). Secrets, PATs, customer PII with no business in memory, and credentials never enter `content`.

## `source_id` patterns

A second ingest with the same `source_id` and same `content` is **idempotent**. A second ingest with the same `source_id` and different `content` is a **conflict** — do not "update" by re-ingesting; ingest the new source under a new `source_id` and `invalidate` the old fact.

Open registry. Project skills add their own. The format is `<kind>:<id>` where `<id>` is whatever the source emits natively.

| Kind | Pattern | When to use |
|---|---|---|
| Mail | `mail:<message-id>` | A single verified email message. |
| Work item | `wi:<id>` | A reconciled work item, post-snapshot. |
| Document | `doc:<path>` | A local document read in full. |
| Ad-hoc | `ad-hoc:<sha256-of-content>` | A bounded summary or excerpt the agent composed. The hash is the dedupe key. |

Never use a free-form string, a UUID minted at capture time, or a counter. The id must round-trip across sessions.

## `t_ref` rules

- ISO 8601 in UTC: `2026-07-20T13:00:00Z`.
- The source's own time, not the moment of capture. For an email, the sent date. For a meeting, the meeting time. For a document, the document's date if present, else the read time as a last resort.
- `as_of` in `assemble_context` compares against `t_ref` (valid time) and `t_ingested` (transaction time) together. The caller's job is to set `t_ref` truthfully; the server does the rest.
- A non-ISO or non-UTC string is an error, not a soft warning — re-issue with the right format before doing anything else.

## CLI flag mapping

The CLI subcommands accept the same fields. The mapping is mechanical — flag name = snake_case field name. A few common ones:

| MCP / params field | CLI flag | Notes |
|---|---|---|
| `source_type` | `--source-type` | Same enum. |
| `source_id` | `--source-id` | Same format rules. |
| `content` | `--content` | Same bounded/source-linked/verified rules. |
| `t_ref` | `--t-ref` | ISO 8601 UTC. |
| `scope` | `--scope` | Default: `org` for ingest, `org` for assemble-context. |
| `project` | `--project` | Optional. |
| `t_ingested` | `--t-ingested` | Optional override. |
| `policy_tags` | `--policy-tag` (repeatable) | One or more. |
| `query` (assemble) | `--query` | Natural language. |
| `as_of` (assemble) | `--as-of` | ISO 8601. |
| `fact_types` (assemble) | `--fact-type` (repeatable) | Filter. |
| `budget` (assemble) | `--budget` | Default 5. |
| `view_mode` (assemble) | `--view-mode` | `current` / `all` / `diff`. |
| `window_start` / `window_end` | `--window-start` / `--window-end` | Temporal range. |
| `context_items` (explain) | `--context-items` | JSON array string from assemble-context output. |
| `fact_id` / `reason` / `t_invalid` (invalidate) | `--fact-id` / `--reason` / `--t-invalid` | All required. |
| `aliases` (resolve) | `--aliases` (repeatable) | One or more. |

When both surfaces are available, prefer MCP for normal agent work; reserve the CLI for one-shot automation, scripts, and the operational modes (`serve` / `watch` / `reembed`).

## `extract` results

Three outcomes, distinguished:

- **Verified facts** — non-empty `facts` with inspected content. The capture is **verified**.
- **Empty facts** — `facts: []` after a real extract call. The episode is recorded; no durable fact was captured. State this as **episode-only**, not as success.
- **Failure** — the call did not complete. Source handles (`source_id`, `episode_id` if any) are preserved; the result is **pending**.

Do not collapse the three into "captured". The distinction is what makes later retrieval honest.
