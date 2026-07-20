# Source Conventions

Detail for the memory capture SOP. Inline the rules, push the catalog here.

## Field contract

The agent supplies these fields. Everything else is server-managed.

| Field | Type | Rule |
|---|---|---|
| `source_type` | enum | One of `email`, `document`, `conversation`, `ad-hoc`. Pick the type that matches the source, not the channel. |
| `source_id` | string | Deterministic, globally unique, and round-trippable. See patterns below. |
| `content` | string | Bounded, source-linked, verified. Not raw tool output, not a draft, not a secret. |
| `t_ref` | ISO 8601 | The source's own timestamp, in UTC (`...Z`). Not the time of capture. |
| `scope` | enum | The narrowest suitable. See [scope guide](scope-guide.md). |
| `project` | string, optional | When the agent knows the project, set it. When it does not, omit. |

`visibility_scope`, `policy_tags`, and other optional metadata are server-managed and not part of the agent contract. If a project requires them, that project's skill defines the convention; this skill does not.

## `source_id` patterns

A second ingest with the same `source_id` and the same `content` is idempotent. A second ingest with the same `source_id` and a different `content` is a **conflict** — do not attempt to "update" by re-ingesting; ingest a new source under a new `source_id` and `invalidate` the old fact.

Open registry of patterns. Project skills add their own; the format is `<kind>:<id>` where `<id>` is whatever the source emits natively.

| Kind | Pattern | When to use |
|---|---|---|
| Mail | `mail:<message-id>` | A single verified email message. |
| Work item | `wi:<id>` | A reconciled work item, post-snapshot. |
| Document | `doc:<path>` | A local document that has been read in full. |
| Ad-hoc | `ad-hoc:<sha256-of-content>` | A bounded summary or excerpt the agent composed. The hash is the dedupe key. |

Never use a free-form string, a UUID minted at capture time, or a counter. The id must round-trip across sessions.

## `t_ref` rules

- ISO 8601 in UTC: `2026-07-20T13:00:00Z`.
- The source's own time, not the moment of capture. For an email, the sent date. For a meeting, the meeting time. For a document, the document's date if present, else the read time as a last resort.
- `as_of` in `assemble_context` compares against `t_ref` (valid time) and `t_ingested` (transaction time) together. The agent's job is to set `t_ref` truthfully; the server does the rest.

## `content` rules

- Bounded. If the source is a 40-page document, ingest a cited excerpt with its location, not the whole document.
- Source-linked. The first sentence or the first metadata field should make provenance obvious.
- Verified. Do not ingest a draft, a paraphrase you are not sure of, or a tool response that has not been confirmed.
- Free of secrets. Tokens, PATs, API keys, customer PII that has no business being in memory, and credentials of any kind do not belong here.

## `extract` results

Three outcomes, distinguished:

- **Verified facts** — non-empty `facts` with inspected content. The capture is **verified**.
- **Empty facts** — `facts: []` after a real `extract` call. The episode is recorded; no durable fact was captured. State this as **episode-only** capture, not as success.
- **Failure** — the call did not complete. Source handles (`source_id`, `episode_id` if any) are preserved; the result is **pending**.

Do not collapse the three into "captured". The distinction is what makes later retrieval honest.
