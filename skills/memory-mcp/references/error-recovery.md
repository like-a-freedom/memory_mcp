# Error Recovery

What to do when an MCP call returns an error or an unexpected shape. The parent skill's `SOP exit gate` requires that failures be reported, not papered over.

## Common errors

| Symptom | Likely cause | Recovery |
|---|---|---|
| `Invalid t_ref` / `Invalid t_invalid` | Non-ISO 8601 or non-UTC string | Re-issue with `YYYY-MM-DDTHH:MM:SSZ` format. The source's own time, not now. |
| `No input` on `extract` | Both `episode_id` and inline `content` missing | Provide one. Prefer `episode_id` from a prior verified `ingest`. |
| `Unknown source_id` | Re-ingest attempted with a new shape, or a typo | Confirm the exact `source_id` from the prior `ingest` response. If the source has changed, ingest under a new `source_id` and `invalidate` the old fact. |
| `Scope denied` | Caller lacks access to the requested scope | Use the narrowest scope the caller can access; for capture, downgrade the audience by re-ingesting at the permitted scope and invalidating the original. |
| `Conflict on source_id` | Same id, different content | Treat as a new source under a new `source_id`; do not attempt to overwrite. |
| Empty `facts: []` | `extract` ran but found no durable facts | Report as **episode-only** capture. The source is recorded; the fact set is empty by design or by extraction limit. |
| Timeout on `extract` | Source too large or extraction limit hit | Split the source into smaller bounded episodes; retry. Do not re-ingest the whole source. |
| `assemble_context` returns nothing | No facts at the requested scope, or query terms miss all stored facts | Widen the scope only if verified evidence requires it; otherwise rephrase the query with the entity's canonical name. A return of nothing is **no durable fact**, not **no truth**. |

## Recovery principles

- **Failures are state, not absence.** A failed `ingest` does not mean no fact exists; it means the capture did not complete. Preserve the source handles (`source_id`, intended `t_ref`, intended `scope`) and report pending status.
- **Pending is honest.** When a step cannot complete, the SOP result is **pending** — never **verified**, never silently dropped. The next session picks up from the preserved handles.
- **Don't smuggle retries into a read path.** A failed `assemble_context` is not a license to call `ingest` to "seed" the answer. If a fact must be captured, end the retrieval and run the capture SOP.
- **Don't widen scope on a hunch.** If `assemble_context` at `team` returns nothing, the absence is informative; widening to `org` should be justified by evidence that the fact lives there, not by the desire to fill a void.
