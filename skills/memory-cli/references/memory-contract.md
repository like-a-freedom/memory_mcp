# Memory Contract

Load this reference before a write, when choosing time boundaries, or
when recovering from a non-success result. Live CLI help remains authoritative
for accepted flags and values.

## Source

- `source_id` is stable and round-trippable. Same ID and content is idempotent;
  same ID with different content is a conflict.
- `content` is bounded, source-linked, verified, and safe to pass through the
  current shell environment. URL/path content is literal; only an explicitly
  authorized `import` reads/fetches external source material.
- `source_type` uses a value accepted by live command help.
- `--t-ref` is the source's valid time in ISO 8601. Omit `--t-ingested`: the public tool rejects caller transaction-time
  overrides. Use `--t-ref` for the source/event date.
- Changed evidence gets a new source identity. Superseded facts are invalidated,
  never deleted.

## Storage boundary

One Active Namespace is bound at server startup (environment configuration, not
per-command input). No ordinary command accepts a `--scope`, `--project`, or
`--namespace` flag, and none should be invented or probed. Use repeatable
`--policy-tag` on ingest for content governance so recall-time policy filtering
has evidence to work with. Empty recall is not authority to widen a boundary.

Credentials never enter arguments or persisted content. Sensitive business data
is eligible only when the server's configured policy explicitly permits it.

## Filesystem ingestion

- Filesystem ingestion is an optional capability of `serve`, activated only by
  `MEMORY_INGESTION_INBOX` (an existing absolute directory, not a symlink).
- Files are ordinary source-labeled data: no elevated trust, no privileged
  policy tags.
- Each distinct set of bytes at a path is one immutable revision; renaming a
  file starts a new lineage; deleting a file never invalidates memory.
- A revision is `processed` only after `ingest → extract` succeeds. Failed revisions are
  eligible for processing again at startup; processing uses up to three attempts; a failed file never stops MCP.
- Supported formats: PDF, DOCX, XLSX, PPTX, Markdown, plain text, and EML.
  Symlinks and unsupported files are skipped silently.
- The `fs-watch` feature is required; official release binaries include it.

## Time and results

Use `--valid-at` for world-valid time and either `--known-at` for committed
knowledge by a timestamp or `--known-revision` for an exact server checkpoint.
The knowledge flags are mutually exclusive. Legacy `--as-of` sets both axes and
cannot accompany a new axis. Omitted axes capture a new request snapshot.
To refine within one snapshot, copy `context.snapshot.valid_at` and
`context.snapshot.known_revision` to the corresponding flags; omit `--as-of` and
`--known-at`. Tokens belong to this namespace/history.

Historical reads support `--view-mode current` only and may be partial. Inspect
completeness, omitted channels, scan truncation and guidance: an empty partial
result or refinement does not establish global absence. Current assembly is a
logical read that can persist access heat.

Read structured `status` and `guidance` before classifying facts. Classify capture
as `verified` (completed extraction and facts inspected), `episode-only`
(completed successful extraction with no facts), or `pending` (partial/pending
extraction, nonzero exit, or unresolved failure). Exit zero can contain pending
extraction; preserve its `episode_id` and follow retry guidance. Classify recall as
`assembled`, and as `cited` only after every used claim has provenance.

After completed successful extraction, an empty fact set confirms no canonical fact capture; source fallback may
still return the episode text. Never treat exit zero, an episode ID, or an empty
result as stronger evidence than the structured response provides.

## Recovery

- Invalid time: correct it to the source's actual ISO 8601 time.
- Source conflict: retain the original and use a new identity for changed
  evidence.
- Completed successful extraction with no facts: report `episode-only`.
- Partial/pending extraction: preserve `episode_id`, report `pending`, and follow
  retry guidance even when the CLI exits zero.
- Nonzero exit or invalid structured output: preserve identifiers and report
  `pending`.
- Empty recall: reframe terms within the same authorized boundary.

After a mutation, require structured output or a subsequent read that
demonstrates persisted state before reporting completion.
