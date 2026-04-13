# Memory MCP

[![Rust](https://img.shields.io/badge/Rust-1.85%2B-orange.svg?logo=rust)](https://www.rust-lang.org)
[![Edition](https://img.shields.io/badge/edition-2024-blue.svg)](https://doc.rust-lang.org/edition-guide/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

`memory_mcp` is a Rust-based Model Context Protocol (MCP) server that gives AI agents a structured long-term memory layer backed by SurrealDB.

It is designed for workflows where agents need more than short-lived chat context: episodic memory, extracted entities and facts, bi-temporal validity, ranked context assembly, and graph-style relationships between people, companies, tasks, and decisions.

## Table of contents

- [Overview](#overview)
- [What it provides](#what-it-provides)
- [Architecture](#architecture)
- [Quick start](#quick-start)
- [Configuration](#configuration)
- [MCP tools](#mcp-tools)
- [Development](#development)
- [Testing](#testing)
- [Project layout](#project-layout)
- [Documentation](#documentation)
- [Contributing](#contributing)
- [License](#license)

## Overview

Memory MCP implements a memory system for AI agents with a few core goals:

- preserve important source material as episodes
- extract entities, facts, and links in a deterministic way
- track knowledge over both valid time and transaction time
- assemble compact, relevant context for downstream reasoning
- support scope-aware retrieval and access filtering

In practice, that means an agent can ingest content such as emails, notes, or working documents, resolve entities consistently, store facts with provenance, and later ask for ranked context instead of replaying entire histories.

## What it provides

- **Bi-temporal knowledge model** for valid time and ingestion time
- **Episode ingestion** for storing raw source material
- **Entity resolution** with alias handling and deterministic IDs
- **Fact extraction** for metrics, promises, and other structured knowledge
- **Context assembly** for ranked retrieval by query, scope, and time cutoff
- **Graph relationships** between episodes, entities, and facts
- **Optional semantic retrieval providers** including in-process `local-candle`
- **Optional local GLiNER NER** for zero-shot entity extraction
- **SurrealDB support** for embedded and remote deployments
- **Optional watch-mode ingestion** for filesystem-backed auto-ingest workflows
- **MCP-native interface** for tool-driven agent workflows
- **Structured logging** with predictable operational behavior

## Architecture

At a high level, the project follows a layered Rust design:

```text
Agent / MCP client
    │
    ▼
Memory MCP server (`src/mcp/`)
    │
    ▼
Memory service layer (`src/service/`)
    │
    ▼
Storage layer (`src/storage.rs` + SurrealDB)
```

### Main modules

| Module | Purpose |
| --- | --- |
| `mcp` | MCP handlers, params, parsers, and tool-facing types |
| `service` | Core business logic for ingest, extract, retrieval, graph operations, and validation |
| `storage` | Database integration and persistence helpers |
| `models` | Shared domain models and request/response types |
| `config` | Environment-driven configuration loading |
| `logging` | Logging setup and log-level utilities |

## Quick start

### Requirements

- Rust 1.85+
- SurrealDB-compatible runtime configuration

### Build

```bash
cargo build --release
```

### Install locally

```bash
cargo install --path .
```

### Run

```bash
cargo run
```

The binary uses stdio transport, which makes it suitable for local MCP client integration.

### Run with environment

```bash
SURREALDB_URL=rocksdb://./data/surreal.db \
SURREALDB_DB_NAME=memory \
SURREALDB_NAMESPACES=org,personal \
SURREALDB_USERNAME=root \
SURREALDB_PASSWORD=root \
RUST_LOG=info \
cargo run --quiet --bin memory_mcp
```

### Filesystem watch mode (optional)

The watch mode turns a directory into a **passive memory intake pipe**: drop or save files and the server auto-ingests them into memory without any manual tool calls.

**Why this exists.** In real workflows, important content already lands on disk — email exports (`.eml`), meeting notes (`.md`, `.docx`), requirements specs, sizing documents. Instead of manually calling `ingest` for each file, the watcher monitors a directory and feeds new or changed files through the full extraction pipeline (NER → entity resolution → fact extraction → embedding) automatically.

**What it does.**

- Recursively watches a directory for file **create** and **modify** events
- Filters to supported file types only; unsupported files are silently skipped
- Deduplicates rapid successive events per file (coalescing)
- Dispatches qualifying files through the same `ingest` → `extract` pipeline used by MCP tool calls
- Logs every step with structured events (visible at `RUST_LOG=info`/`debug`/`trace`)

**Supported file types.**

| Extension | Format | Extracted content |
|-----------|--------|-------------------|
| `.pdf` | PDF | Text content (pages, paragraphs) |
| `.docx` | Word document | Body text, headings, tables |
| `.xlsx` | Spreadsheet | Cell values, sheet structure |
| `.pptx` | Presentation | Slide text, speaker notes |
| `.md`, `.markdown` | Markdown | Headings, lists, code blocks |
| `.txt` | Plain text | Raw text content |
| `.eml` | Email message | Subject, sender, recipients, body, date |

Files with other extensions (`.json`, `.png`, `.zip`, etc.) are **silently skipped**.

**User scenario.**

<details>
<summary><strong>Example: auto-ingest a project inbox</strong></summary>

```bash
# Terminal 1 — start the MCP server (stdio, for VS Code / Copilot)
SURREALDB_URL=rocksdb://./data/surreal.db \
SURREALDB_NAMESPACES=org \
SURREALDB_USERNAME=root \
SURREALDB_PASSWORD=root \
RUST_LOG=info \
cargo run --quiet --bin memory_mcp

# Terminal 2 — start the watcher on a project inbox
cargo run --features cli-watch --quiet -- \
  watch ~/projects/atlas/inbox \
  --project atlas \
  --scope org \
  --interval 5
```

Now any file dropped or saved in `~/projects/atlas/inbox/` is automatically ingested:

```bash
# Drop an email export
cp ~/Downloads/kaspersky_july_2025.eml ~/projects/atlas/inbox/

# Save a requirements spec
echo "# Air-gapped deployment requirement..." > ~/projects/atlas/inbox/airgap_req.md

# Drop a sizing document
cp ~/Documents/hw_sizing.xlsx ~/projects/atlas/inbox/
```

Within `--interval` seconds, each file is:
1. Detected by the watcher
2. Parsed (format-specific extraction)
3. Ingested as an episode with `source_id = "watch:<path>"`
4. Available for `extract` and `assemble_context` queries

No manual `ingest` tool call needed.
</details>

**How it works internally.**

<details>
<summary><strong>Architecture flow</strong></summary>

```
CLI: memory_mcp watch <dir> [--project X] [--scope Y] [--interval Z]
  │
  ▼
FsWatcher::run_with_interval(dir, project, scope, interval, service)
  │
  ├─ Validate: directory must exist and be readable
  ├─ Initialize: notify::RecommendedWatcher (polling mode)
  ├─ Watch: dir recursively for filesystem events
  │
  └─ EVENT LOOP (blocks on rx.recv())
       │
       ├─ Event arrives (Create / Modify / Remove / Access / …)
       │
       ├─ Filter: keep only Create + Modify events
       ├─ Filter: keep only supported file types (7 formats)
       │
       ├─ Dedup: if same file triggered an ingest within
       │         --interval seconds → skip (logged at trace)
       │
       ├─ Determine metadata:
       │   • source_id = "watch:<file_path>"
       │   • source_type = "email" (.eml) or "document" (all others)
       │   • project = CLI flag value
       │   • scope = CLI flag value
       │
       ├─ Dispatch: service.ingest(IngestRequest { content: <file_path> })
       │   └─ Internally: read file → detect format → extract text → chunk
       │
       ├─ Log: watcher.ingest_complete (with episode_id) at Info
       │
       └─ On error: log at Error and TERMINATE the watcher (fail-fast)
```
</details>

**Deduplication behavior.**

<details>
<summary><strong>How rapid saves are handled</strong></summary>

When you save a file, editors often fire multiple filesystem events in quick succession (write + metadata + timestamp). The watcher prevents duplicate ingests:

- Each file's **canonical path** (symlinks resolved, `..` normalized) is tracked in a `HashMap`
- If the same file triggers another event **within `--interval` seconds** of its last ingest, the new event is **skipped**
- Skipped events are logged at `trace` level with reason `interval_dedup`

Example with `--interval 5`:
```
12:00:00 — note.md modified → ingested ✓
12:00:01 — note.md modified again → skipped (dedup, 1s < 5s)
12:00:02 — note.md modified again → skipped (dedup, 2s < 5s)
12:00:06 — note.md modified again → ingested ✓ (6s ≥ 5s)
```

The `--interval` flag serves **dual purposes**: it controls both the poll frequency (how often notify scans the directory) **and** the dedup window (minimum time between ingests of the same file).
</details>

**Command-line reference.**

<details>
<summary><strong>Flags and defaults</strong></summary>

```
memory_mcp watch <dir> [OPTIONS]

Required:
  <dir>              Directory to watch (must exist and be readable)

Optional:
  --project <name>   Attach ingested episodes to a project (default: none)
  --scope <scope>    Scope for namespace resolution (default: "org")
  --interval <secs>  Poll interval + dedup window in seconds (default: 2, min: 1)
```

Important notes:
- The `watch` subcommand requires the `cli-watch` feature. Without it, the binary returns an error.
- The watcher is **fail-fast**: any ingest error terminates the entire watch loop.
- The watcher **does not diff content** — every qualifying event triggers a full re-ingest of the file.
- `Remove`, `Access`, and `Metadata` change events are ignored.
</details>

**Logging during watch.**

<details>
<summary><strong>What to expect at each log level</strong></summary>

| Level | Watch events you'll see |
|-------|------------------------|
| `info` | `watcher.ready` (startup), `watcher.ingest_complete` (with `episode_id`) |
| `debug` | `watcher.ingest_dispatch` (file path, source_type, project, scope) |
| `trace` | `watcher.event_skipped` (dedup reason, elapsed vs interval) |
| `warn` | — (none specific to watch) |
| `error` | `watcher.ingest_error` (fatal — watcher terminates) |

Example info-level output for a successful ingest:
```
[2026-04-13T12:00:00.123Z] INFO  req=-       op=watcher.ingest_complete  episode_id=episode:abc123  path=watch:/inbox/note.md  source_type=document
```
</details>

### VS Code MCP host example

If you run the server directly from this workspace, a stdio host configuration can point at Cargo:

```json
{
    "mcpServers": {
        "memory-mcp": {
            "command": "cargo",
            "args": ["run", "--quiet", "--bin", "memory_mcp"],
            "cwd": "/path/to/memory_mcp",
            "env": {
                "SURREALDB_URL": "rocksdb://./data/surreal.db",
                "SURREALDB_DB_NAME": "memory",
                "SURREALDB_NAMESPACES": "org,personal",
                "SURREALDB_USERNAME": "root",
                "SURREALDB_PASSWORD": "root",
                "RUST_LOG": "info"
            }
        }
    }
}
```

After `cargo build --release` or `cargo install --path .`, you can switch `command` to `./target/release/memory_mcp` or `memory_mcp` respectively.

## Configuration

Configuration is loaded from environment variables.

### Required variables

| Variable | Required | Description |
| --- | --- | --- |
| `SURREALDB_DB_NAME` | Yes | Database name |
| `SURREALDB_NAMESPACES` | Yes | Comma-separated namespace list |
| `SURREALDB_USERNAME` | Yes | Database username |
| `SURREALDB_PASSWORD` | Yes | Database password |
| `SURREALDB_URL` | Yes for remote mode | SurrealDB connection URL |

### Optional variables

| Variable | Description |
| --- | --- |
| `SURREALDB_EMBEDDED` | Set to `true` to use embedded mode |
| `SURREALDB_DATA_DIR` | Custom embedded data directory |
| `RUST_LOG` | Logging level such as `trace`, `debug`, `info`, `warn`, or `error` |
| `QUERY_LOGGING_ENABLED` | Set to `true` to persist `assemble_context` analytics rows into `query_log` (default: `false`) |
| `QUERY_LOG_RETENTION_DAYS` | Days to retain persisted `query_log` analytics before best-effort pruning (default: `90`) |
| `LIFECYCLE_ENABLED` | Enable background lifecycle jobs (`true`/`false`, default: `false`) |
| `LIFECYCLE_DECAY_INTERVAL_SECS` | Decay worker interval in seconds (default: `3600`) |
| `LIFECYCLE_ARCHIVAL_INTERVAL_SECS` | Archival worker interval in seconds (default: `86400`) |
| `LIFECYCLE_DECAY_THRESHOLD` | Confidence threshold for fact invalidation (default: `0.3`) |
| `LIFECYCLE_ARCHIVAL_AGE_DAYS` | Days before archiving episodes (default: `90`) |
| `LIFECYCLE_DECAY_HALF_LIFE_DAYS` | Half-life in days for decay computation (default: `365`) |
| `EMBEDDINGS_ENABLED` | Enable semantic retrieval providers (default: `false`) |
| `EMBEDDINGS_PROVIDER` | Embedding backend: `local-candle`, `openai-compatible`, or `ollama` |
| `EMBEDDINGS_MODEL` | Model identifier for the selected embedding provider |
| `EMBEDDINGS_MODEL_DIR` | Optional local cache directory for `local-candle` |
| `EMBEDDINGS_MAX_TOKENS` | Max token budget before `local-candle` chunks long inputs |
| `EMBEDDINGS_TIMEOUT_SECS` | Timeout for remote embedding calls |
| `EMBEDDINGS_SIMILARITY_THRESHOLD` | Minimum cosine similarity for semantic matches |
| `EMBEDDINGS_API_KEY` | Optional bearer token for OpenAI-compatible providers |
| `NER_PROVIDER` | Entity extraction backend: `regex`, `anno`, or `local-gliner` |
| `NER_MODEL` | HuggingFace repo for `local-gliner` |
| `NER_MODEL_DIR` | Optional local cache directory for `local-gliner` |
| `NER_LABELS` | Comma-separated runtime labels for `local-gliner` |
| `NER_THRESHOLD` | Confidence threshold for `local-gliner` acceptance |
| `NER_BATCH_SIZE` | Batch size for `local-gliner` inference |

### Example

```bash
SURREALDB_DB_NAME=memory
SURREALDB_NAMESPACES=org,personal
SURREALDB_USERNAME=root
SURREALDB_PASSWORD=root
SURREALDB_URL=ws://127.0.0.1:8000/rpc
SURREALDB_EMBEDDED=false
RUST_LOG=info
QUERY_LOGGING_ENABLED=false
QUERY_LOG_RETENTION_DAYS=90

# Lifecycle background jobs (optional)
LIFECYCLE_ENABLED=true
LIFECYCLE_DECAY_INTERVAL_SECS=3600
LIFECYCLE_ARCHIVAL_INTERVAL_SECS=86400
LIFECYCLE_DECAY_THRESHOLD=0.3
LIFECYCLE_ARCHIVAL_AGE_DAYS=90
# LIFECYCLE_DECAY_HALF_LIFE_DAYS=365

# Optional local model configuration
# EMBEDDINGS_ENABLED=true
# EMBEDDINGS_PROVIDER=local-candle
# EMBEDDINGS_MODEL=intfloat/multilingual-e5-small
# EMBEDDINGS_MODEL_DIR=./data/models/intfloat/multilingual-e5-small
# NER_PROVIDER=local-gliner
# NER_MODEL=urchade/gliner_multi-v2.1
```

### Embedding providers and switching

The server supports three embedding backends, controlled by `EMBEDDINGS_PROVIDER`:

| Provider | What it is | Default dimension | Requires network? |
| --- | --- | --- | --- |
| `local-candle` | In-process BERT model via Candle (Rust ML) | 384 | Only for first download |
| `openai-compatible` | External OpenAI-compatible HTTP API | 1536 (configurable) | Yes, every call |
| `ollama` | External Ollama HTTP API | 1536 (configurable) | Yes, every call |

#### How it works at startup

1. The server reads `EMBEDDINGS_PROVIDER` and related variables **once** at startup.
2. A single embedding provider is created and stored as `Arc<dyn EmbeddingProvider>`.
3. **The provider cannot be changed without restarting** — there is no runtime hot-swap.
4. For `local-candle`, the model files are downloaded from HuggingFace Hub on first use and cached locally (default: `<data_dir>/models/<model_name>/`). Subsequent restarts reuse the cached files.

#### What happens when you switch providers

To switch, you change the environment variables and **restart the server**. There is no automatic data migration. Here is what to expect:

**Existing embeddings stay in the database.** Previously stored embedding vectors are never deleted, re-generated, or converted. They remain as-is in the `fact.embedding` column.

**The HNSW index is rebuilt with the new dimension.** On startup, the schema migration applies the current `dimension` value to `DEFINE INDEX ... HNSW DIMENSION N`. The index structure is re-created to match the new provider's output.

**Semantic retrieval will return poor results until facts are re-extracted.** Embedding vectors from different models live in different semantic spaces — even if two models produce the same dimension (e.g., both 384), their cosine similarity scores are not comparable. After a switch, old vectors and new query embeddings are computed by different models, so similarity scores are essentially meaningless.

#### Scenario: `local-candle` → `openai-compatible`

| Step | What happens |
| --- | --- |
| 1. Change `EMBEDDINGS_PROVIDER=openai-compatible`, set `EMBEDDINGS_BASE_URL`, restart | New HTTP-based provider created; BERT model released from memory |
| 2. HNSW index re-created with new dimension (e.g., 1536) | Old 384-dim vectors still in DB but incompatible with new index |
| 3. New `extract` calls embed facts with the OpenAI model | New 1536-dim vectors stored alongside old 384-dim ones |
| 4. `assemble_context` semantic search | Only newly embedded facts will produce meaningful similarity scores; old facts will be filtered out by the similarity threshold |

#### Scenario: `openai-compatible` → `local-candle`

| Step | What happens |
| --- | --- |
| 1. Change `EMBEDDINGS_PROVIDER=local-candle`, restart | BERT model loaded from local cache (no re-download if already cached) |
| 2. HNSW index re-created with dimension 384 | Old 1536-dim vectors remain in DB but are incompatible |
| 3. New `extract` calls embed facts with BERT | New 384-dim vectors stored |
| 4. `assemble_context` semantic search | Only newly embedded facts produce meaningful results |

#### Scenario: `local-candle` → external → `local-candle`

Each restart creates a completely fresh provider instance:

1. **Start with `local-candle`** → model cached, 384-dim vectors stored.
2. **Restart with `openai-compatible`** → BERT released, 1536-dim vectors stored. Old 384-dim vectors orphaned.
3. **Restart with `local-candle` again** → model re-loaded from cache (fast, no re-download), 384-dim HNSW index re-created. New facts get fresh 384-dim vectors. The 1536-dim vectors from step 2 are now orphaned.

**Nothing is lost** — old vectors remain in the database. But they are **not usable** for semantic search with the current provider.

#### Similarity threshold

The `EMBEDDINGS_SIMILARITY_THRESHOLD` (default `0.7`) filters semantic search results: only facts with cosine similarity ≥ threshold are returned. After a provider switch, this threshold effectively filters out **all** old facts because cross-provider similarity scores are meaningless.

#### Recommended procedure after switching

To restore full semantic retrieval quality after a provider change:

1. Switch the environment variables and restart.
2. Re-run `extract` on all episodes whose facts you want to search semantically. This re-embeds facts with the new provider.
3. Optionally, invalidate old facts that were embedded with the previous provider to avoid mixing incompatible vectors.

If you only use **lexical** (BM25/FTS) retrieval and **graph-expanded** context assembly, the provider switch has **no impact** on those retrieval tiers — they do not use embeddings.

### Query analytics logging

Persisted query analytics are **optional** and **disabled by default**.

When `QUERY_LOGGING_ENABLED=true`, successful `assemble_context` calls write a row to the `query_log` table with:

- `scope`
- `query`
- `project`
- `view_mode`
- `result_count`
- `latency_ms`
- `retrieval_tier`
- `cache_hit`
- `logged_at`

Old `query_log` rows are pruned with a best-effort retention pass after successful writes. By default, rows older than `90` days are deleted; override this with `QUERY_LOG_RETENTION_DAYS=<days>`.

This switch only controls database-backed query analytics. Regular runtime logs still follow `RUST_LOG`.

### Logging levels and what they cover

`memory_mcp` emits structured logs across the plan-added functionality using the standard levels below:

- `info` — lifecycle milestones and successful high-level operations such as `ingest`, `extract`, `assemble_context`, watcher startup, watcher ingest completion, and community rebuild passes
- `debug` — feature-path decisions such as document ingest transport detection (`file`/`directory`/`url`/`inline`), project/view-mode selection, graph insight assembly, hub/community map building, and successful `query_log` writes when enabled
- `trace` — fine-grained diagnostics such as cache misses/sets, `query_log` skips when disabled, watcher dedup skips, retrieval-tier summaries, appended `experience` facts, and per-namespace community rebuild details
- `warn` — recoverable issues such as unknown `view_mode` fallback, access-heat tracking failures, query analytics write failures, and degraded worker passes
- `error` — terminal failures such as watcher ingest errors or process-level startup/serve failures

Recommended presets:

- `RUST_LOG=info` for normal local/server usage
- `RUST_LOG=debug` when validating new ingest/view-mode/graph behavior
- `RUST_LOG=trace` when debugging retrieval tiers, cache behavior, or watcher dedup decisions

An `.env` file already exists in the repository root, so you can keep local values there if your MCP host or shell loads it.

## MCP tools

The public MCP surface is centered on a small set of high-value operations rather than endpoint-by-endpoint plumbing.

| Tool | Purpose |
| --- | --- |
| `ingest` | Store an episode with source metadata and timestamps |
| `extract` | Extract entities, facts, and links from an episode or raw content |
| `resolve` | Canonicalize an entity name and aliases into a stable entity record |
| `assemble_context` | Return ranked memory context for a query |
| `explain` | Expand context items with source citations and multi-source provenance |
| `invalidate` | Mark a fact as no longer valid as of a given time |
| `open_app` | Launch an optional MCP app session and return a session-backed resource URI |
| `app_command` | Execute coarse-grained actions against an open MCP app session |

When the MCP host supports resources, the server also exposes app discovery and session resources such as `ui://memory/apps` and `ui://memory/app/{app}/{session_id}` for inspector, diff, ingestion review, lifecycle, and graph views.

### `explain` Multi-Source Provenance

The `explain()` operation returns complete provenance lineage for each fact:

- **Direct sources** — episodes that directly generated the fact
- **Linked sources** — episodes connected via shared entities

**Returns:**
- `all_sources`: Array of provenance sources including:
  - `episode_id`: Source episode identifier
  - `episode_content`: Excerpt from the episode
  - `episode_t_ref`: Episode timestamp
  - `relationship`: "direct" (created fact) or "linked" (via entity)
  - `entity_path`: Path from fact to episode via entity (if linked)

This enables full audit trails, understanding of information propagation, and building trust through transparency.

This design lines up with the intent-driven MCP guidance reflected in the docs: fewer tools, clearer semantics, better outcomes.

### Adaptive Memory Features

As of 2026-03-27, `memory_mcp` implements adaptive memory alignment with SOTA research:

- **Fact-augmented index keys**: Entity names, aliases, and temporal markers (month-year, ISO dates) indexed at ingest for enriched BM25 retrieval. FTS matches on both `content` and `index_keys`.

- **Heat-aware lifecycle**: Recently-accessed facts protected from decay/archival via `access_count` and `last_accessed` fields. Retrieval increments by 1, explain increments by 3 (stronger signal).

- **Timeline retrieval**: `assemble_context` supports `view_mode=timeline` with optional `window_start`/`window_end` for chronological queries. Results sorted by `t_valid` (oldest first).

- **LongMemEval-style acceptance tests**: Coverage for multi-session reasoning, temporal reasoning, knowledge update, abstention, and direct fact lookup.

See `docs/superpowers/specs/2026-03-27-sota-memory-alignment-design.md` for target-state design and `docs/MEMORY_SYSTEM_SPEC.md` for current runtime contract.

## Development

### Daily commands

```bash
cargo check
cargo fmt
cargo clippy -- -D warnings
cargo doc --no-deps
```

### Binary entry points

- `src/main.rs` — main MCP server binary

MCP input/output schemas are exposed by the server itself through the protocol's tool metadata and remain regression-covered by the schema tests under `src/mcp/`.

## Testing

Run the full test suite:

```bash
cargo test
```

Useful narrower runs:

```bash
cargo test --test service_integration
cargo test --test service_acceptance
cargo test --test tools_e2e
```

Verified in this remediation pass:

- `cargo test semantic_scaffolding --test service_integration` → `2 passed; 0 failed`
- `cargo test --test service_acceptance` → `11 passed; 0 failed`
- `cargo test --test service_integration` → `11 passed; 0 failed`

Coverage output is stored under `coverage/` when generated with Tarpaulin.

## Project layout

```text
.
├── AGENTS.md
├── Cargo.toml
├── README.md
├── docs/
├── scripts/
├── src/
│   ├── mcp/
│   ├── service/
│   ├── config.rs
│   ├── lib.rs
│   ├── logging.rs
│   ├── main.rs
│   ├── models.rs
│   └── storage.rs
└── tests/
```

## Documentation

- [`docs/MEMORY_SYSTEM_SPEC.md`](docs/MEMORY_SYSTEM_SPEC.md) — full system specification
- [`docs/SIMPLIFIED_SEARCH_REDESIGN_SPEC.md`](docs/SIMPLIFIED_SEARCH_REDESIGN_SPEC.md) — target-state spec for the upcoming breaking search simplification
- [`docs/INTENT_DRIVEN_MCP_DESIGN_GUIDE.md`](docs/INTENT_DRIVEN_MCP_DESIGN_GUIDE.md) — curated references for intent- and skills-driven MCP design
- [`docs/security-hardening-roadmap.md`](docs/security-hardening-roadmap.md) — current query-surface inventory, deployment assumptions, and remaining hardening work

## Contributing

This repository follows the conventions in [`AGENTS.md`](AGENTS.md).

In particular:

- keep public APIs stable unless a change is explicitly requested
- avoid introducing dependencies without approval
- prefer typed errors and deterministic behavior
- run formatting, clippy, and tests before considering work done

## License

This project is licensed under the **MIT** license. See [`LICENSE`](LICENSE) for details.
