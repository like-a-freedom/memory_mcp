# Memory CLI Command Reference

Load this reference when constructing commands or handling operational modes.
Run `memory_mcp <subcommand> --help` before relying on an optional flag; live
help is authoritative.

## One-shot commands

| Command | Required input |
|---|---|
| `memory_mcp ingest` | `--source-type`, `--source-id`, exactly one of `--content` / `--content-stdin`, `--t-ref`; optional repeatable `--policy-tag` |
| `memory_mcp extract` | `--episode-id`, or inline content plus its source metadata |
| `memory_mcp resolve` | `--entity-type`, `--canonical-name`; aliases are repeatable |
| `memory_mcp assemble-context` | `--query`; optional fact type, independent valid/knowledge time, budget, view mode, temporal windows |
| `memory_mcp explain` | `--context-items` containing the JSON array from assembly |
| `memory_mcp invalidate` | `--fact-id`, `--reason`, `--t-invalid` |

Flag names are kebab-case forms of the shared snake_case parameter names.
Repeatable values use repeated flags. Assembly supports `--valid-at` and either
`--known-at` or `--known-revision`. Legacy `--as-of` sets both axes and cannot be
combined with those flags. See the memory contract for partial historical results
and repeatable snapshots. No ordinary command
accepts a `--scope`, `--project`, or `--namespace` flag: the Active Namespace is
server startup configuration and is never selected per command.

One-shot commands print structured responses to stdout. Capture stderr and the
exit status independently. Validation and configuration failures are terminal
until their cause is corrected; do not parse diagnostic text as a successful
response.

`resolve` invokes resolve-or-create. It may persist a new entity or aliases and
therefore belongs to the canonicalize workflow, not recall.

## Operational commands

- `memory_mcp serve` starts the stdio MCP server. With no subcommand, the binary
  also defaults to server mode.
- Filesystem ingestion runs inside `serve`: set `MEMORY_INGESTION_INBOX` to an
  existing absolute directory to activate it (`fs-watch` feature required;
  official release binaries include it).
- `memory_mcp reembed` rebuilds fact embeddings. `--max-failures 0` is
  fail-fast; `--retry-failed` limits work to facts recorded as failed by a prior
  run.

`reembed` is interactive in a TTY, continues within its configured failure
budget, and uses terminal states `running`, `completed`, `completed_with_errors`,
`failed`, and `interrupted`.

Hidden lifecycle commands are implementation interfaces for installed hooks,
not public commands for ad-hoc agent use.

## Literal capture and explicit operator import

`ingest --content` stores the supplied bytes as text, including any URL/path.
`--content-stdin` reads UTF-8 stdin, up to 8 MiB, without shell argument expansion.
Omit `--t-ingested`: server transaction time is authoritative and overrides are
rejected. `extract --episode-id` reuses stored content; inline `--content`/`--text`
ingests a new episode first. Free-form extraction is heuristic and can return
facts, a fallback note or no facts. Inspect the result rather than assuming any
of these outcomes. Prefer verified structured summaries for explicit decisions.

Only use operator import when the external read is explicitly authorized:

```text
memory_mcp import file --root <absolute-directory> --path <relative-file> --source-type document --t-ref <ISO8601>
memory_mcp import url --url <public-HTTPS-URL> --source-type web --t-ref <ISO8601>
```

The importer generates the source ID and ingests parsed content; extract the
returned episode separately. File reads are root-confined, refuse traversal and symlink escapes outside the
root, and cap input at 4 MiB. URL import refuses credentials, redirects and
non-public destinations and caps the response at 4 MiB. Import is not a way to
bypass a refused credential-bearing write or to grant an agent arbitrary access.

## Evidence and bounded provenance

Extraction returns statement/quote pairs by default within a shared 4 KiB JSON
budget. Missing pairs or overflow omit all pairs and set `fact_evidence_omitted`;
this is not an empty-fact verdict. `extract --compact` opts out of inline
statement/quote evidence; use the default when that evidence is needed, or explain
the returned item separately. Claim rollout
defaults to `evidence`; configured stages and unknown role/coverage still matter.

For `--context-items`, JSON-encode selected assembly items whole, retaining
top-level `source_episode`, `fact_id` and provenance. Time selectors belong to
`assemble-context`, not `explain`; source `t_ref` is not a retrieval snapshot.

`explain` accepts `--item-offset`, `--source-offset`, `--source-limit-scalars`
(maximum 8000, requires source offset), and `--provenance-offset`. Source and
provenance continuations need exactly one selected item. The envelope is bounded
to 32 KiB, default source context to 1200 Unicode scalars, provenance pages to 20
sources. Follow response `next_offset`, `citation_context_next_offset` and
`provenance_next_offset`; offsets count Unicode scalars for source text.
`quote_*` flags describe the selected item's supplied `quote`: when
`quote_supplied=false`, `quote_verified=false` is not a failed comparison.
`citation_context_exact=true` identifies an exact source window; quote only text
actually present in it, preserving its scope and completeness limits. For a
supplied quote inspect `quote_verified`, `quote_contained_in_window` and
`quote_complete`. Exact text alone does not prove a claim or whole-source coverage.
Keep fact identity separate from quote verification: `quote_verified=false`
does not turn a `fact:…` into an episode fallback. Report fallback only when
the returned item identifies it. In a historical final answer include the
actual `context.snapshot` boundary; source `t_ref` cannot substitute for it.
A partial response is not evidence that omitted content is absent. Assembly is
a logical read and can persist fact-access heat.
