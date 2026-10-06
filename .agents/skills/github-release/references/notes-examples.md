# Release-note examples

The notes are the GitHub release body: English, short bulleted groups, written for someone deciding whether to upgrade. These examples are drawn from this repo's own changes so the register matches what a reader here expects.

## A full body

```markdown
No API, tool-surface or configuration changes — deployments upgrade with no operator action.

**Breaking changes**
- The `compact` tool no longer accepts `mode`; compaction is automatic. Callers passing `mode` get a validation error.

**Customer-facing changes**
- HTTP embeddings and reembed now work: `EMBEDDINGS_*` are read by `memory_mcp_http` instead of only by the stdio binary.
- `QUERY_LOGGING_ENABLED`, `MEMORY_CLAIM_*`, `MEMORY_LOG_FILE` and `LIFECYCLE_ENABLED` are honoured by the HTTP profile; they were silently ignored before.

**Behaviour changes**
- `MEMORY_CLAIM_ROLLOUT_STAGE` with an invalid value now fails startup. It used to be ignored, leaving the default stage in place.
- The HTTP embedding backfill runs at most once every 60 seconds instead of on every one-second scheduler tick.
- `embedding.backfill_started` reports at `debug`, not `info`, when a pass finds nothing to do.

**Fixed issues**
- An HTTP tenant whose vectors sat at another provider's width was paid-for and then rejected on every tick; it is now declined before the provider call.
- A model dimension that did not match `SURREALDB_EMBEDDING_DIMENSION` resolved at the wrong width, so semantic search silently degraded.

**Other**
- New `http.lifecycle.*` operations in the log when the decay/archival passes fail for a tenant.
```

## Item-level good vs bad

| Bad | Why | Good |
| --- | --- | --- |
| `Refactored the config layer so both profiles share one parser.` | Mechanism, no user-visible effect. | Delete it — internal-only changes get no bullet. |
| `Fixed a bug in the embedding code.` | Names neither the symptom nor the fix; a reader cannot tell if it is their bug. | `Semantic search returned nothing after enabling embeddings on a namespace indexed at 1536; the index is now re-declared at the configured width.` |
| `Changed logging behaviour.` | Vague; which events, and what changed? | `embedding.backfill_started now logs at debug when there is nothing to backfill, so an idle deployment is silent at info.` |
| `Added lifecycle support.` | Does not say what it does or that it was previously broken. | `LIFECYCLE_ENABLED now runs decay, archival and community passes in HTTP; the flag was read but had no effect.` |
| `BREAKING: everything about config changed.` | If everything is breaking, say what a caller must do. | `MEMORY_CLAIM_CANDIDATE_PAGE_SIZE must be a positive integer; a non-numeric value is now a startup error.` |

## Notes on voice

- **Tense.** Present for what the release now does ("the flag is honoured"), past for what was wrong ("it used to be ignored"). Do not narrate a journey.
- **Person.** No "we"/"I". The reader does not care who changed it.
- **Hedges.** Cut "may", "should mostly", "various". State the fact or leave it out.
- **Numbers.** Keep them when they are the point (`at most once every 60 seconds`, `2048`-dimension), because they are the detail an operator acts on.
- **Length.** If a group runs past ~5 bullets, the release is large — that is fine, but each bullet still fits one line in the code block.
