# Memory MCP

[![Rust](https://img.shields.io/badge/Rust-1.88%2B-orange.svg?logo=rust)](https://www.rust-lang.org)
[![Edition](https://img.shields.io/badge/edition-2024-blue.svg)](https://doc.rust-lang.org/edition-guide/)
[![License](https://img.shields.io/badge/license-MIT-blue.svg)](LICENSE)

> ⚠️ **Disclaimer:** This project is **not production-ready**. It is currently an **educational project** intended for learning, experimentation, and research purposes only. Do not use it in production environments or for critical workloads.

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

Memory MCP implements a memory system for AI agents with core goals:

- preserve important source material as episodes
- extract entities, facts, and links in a deterministic way
- track knowledge over both valid time and transaction time
- assemble compact, relevant context for downstream reasoning
- support scope-aware retrieval and access filtering

In practice, an agent can ingest emails, notes, or working documents, resolve entities consistently, store facts with provenance, and later ask for ranked context instead of replaying entire histories.

## What it provides

- **Bi-temporal knowledge model** for valid time and ingestion time
- **Episode ingestion** for storing raw source material
- **Entity resolution** with alias handling and deterministic IDs
- **Fact extraction** for metrics, promises, and other structured knowledge
- **Context assembly** for ranked retrieval by query, scope, and time cutoff
- **Graph relationships** between episodes, entities, and facts
- **Optional semantic retrieval providers** including in-process `local-candle`
- **Pluggable NER backends** for entity extraction: `regex`, `anno`, or local zero-shot GLiNER (selectable via `NER_PROVIDER`)
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

- Rust 1.88+ only when compiling from source
- No external SurrealDB service is required for the default embedded mode

### First run with a release binary

1. Download the release asset for your platform and verify its accompanying SHA-256 checksum. Rename it to `memory_mcp` (or create a symlink with that name); the Windows asset already includes `.exe`.
2. Put the renamed executable on `PATH`.
3. Run `memory_mcp init` for the default VS Code snippet, or pass one of the exact targets `claude-desktop`, `codex`, `zed`, or `env`.
4. Copy the printed host-native snippet into the indicated configuration file.
5. Ingest one source, run `extract --episode-id <episode-id>`, then run `assemble-context` to verify a real fact is recalled.

The default path needs no environment variables, configuration file, external database, API key, network request, or model download. It uses a user-owned embedded database, Anno extraction, and lexical/graph retrieval immediately. `memory_mcp init` prints configuration only: it does not edit host files, change environment variables, start a database, download models, or access the network.

### Install from source (fallback)

A Rust toolchain is needed only when a prebuilt release is unavailable:

```bash
cargo install --path crates/memory-mcp --locked
```

This builds the same full-capability application as the release binary; it is not a reduced onboarding build.

### Measuring time-to-value

The clean-machine harness measures the path from the selected persona's start to a
real fact recalled by `assemble-context`; it does not measure GUI host startup or
memory quality. It uses isolated `HOME`, `XDG_DATA_HOME`, `CARGO_HOME`, and working
directories and prints machine-readable timings with median and p90 aggregates:

```bash
scripts/measure_ttv.sh --binary ./target/release/memory_mcp --persona release-binary --repeat 5
scripts/measure_ttv.sh --binary ./target/release/memory_mcp --persona host-config-user --repeat 5
scripts/measure_ttv.sh --cargo-install --source . --persona rust-user --repeat 5
```

The fixture is a summary-like `requirement` episode because the existing extractor
intentionally limits note fallback facts to summary-capable source types. The
validator rejects malformed responses, empty fact arrays, and episode-only fallback
items, so a run is successful only when a persisted fact—not an episode fallback—is
recalled. Installation, host-snippet preparation, storage initialization, episode
write, extraction, and fact recall are reported separately. A median total of
`<= 300` seconds is the measured target, not a guarantee; the first clean rust-user
run on this macOS workspace took `544.098` seconds, with `542.634` seconds spent in
isolated compilation/install and approximately `1.46` seconds in the application
path.

### Run

```bash
cargo run --release -- serve
# or
make serve-release
```

For local NER workloads, run the MCP server from a release build. The development
profile leaves the `memory_mcp` crate at `opt-level = 0`; dependency code is optimized,
but GLiNER window orchestration and span enumeration are not. Performance claims and
timeout investigations are valid only for release builds. Use `cargo run` only for
development and functional debugging.

The binary uses stdio transport, which makes it suitable for local MCP client integration.

### Run with environment

The default embedded mode needs no `SURREALDB_*` variables. To select a remote
SurrealDB explicitly, use one of the supported remote schemes (`ws`, `wss`,
`http`, or `https`) and provide non-empty credentials:

```bash
SURREALDB_URL=ws://127.0.0.1:8000/rpc \
SURREALDB_EMBEDDED=false \
SURREALDB_DB_NAME=memory \
SURREALDB_NAMESPACES=org,personal \
SURREALDB_USERNAME=<your-remote-username> \
SURREALDB_PASSWORD=<your-remote-password> \
RUST_LOG=info \
cargo run --quiet --bin memory_mcp
```

`mem://` and `rocksdb://` are not remote URL schemes. For an explicit local
RocksDB location, set `SURREALDB_DATA_DIR`; otherwise the server uses a
user-owned data directory by default.

### Filesystem watch mode (optional)

The watch mode turns a directory into a **passive memory intake pipe**: drop or save files and the server auto-ingests them without manual tool calls.

In real workflows, important content already lands on disk — email exports (`.eml`), meeting notes (`.md`, `.docx`), requirements specs, sizing documents. Instead of manually calling `ingest` for each file, the watcher monitors a directory and feeds new or changed files through the full extraction pipeline (NER → entity resolution → fact extraction → embedding) automatically.

**What it does**

- Recursively watches a directory for file **create** and **modify** events
- Filters to supported file types only; unsupported files are silently skipped
- Deduplicates rapid successive events per file (coalescing)
- Dispatches qualifying files through the same `ingest` → `extract` pipeline used by MCP tool calls
- Logs every step with structured events (visible at `RUST_LOG=info`/`debug`/`trace`)

**Supported file types**

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

**User scenario**

<details>
<summary><strong>Example: auto-ingest a project inbox</strong></summary>

```bash
# Terminal 1 — start the MCP server (stdio, for VS Code / Copilot)
RUST_LOG=info cargo run --quiet --bin memory_mcp -- serve

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
cp ~/Downloads/acme_july_2025.eml ~/projects/atlas/inbox/

# Save a requirements spec
echo "# Air-gapped deployment requirement..." > ~/projects/atlas/inbox/airgap_req.md

# Drop a sizing document
cp ~/Documents/hw_sizing.xlsx ~/projects/atlas/inbox/
```

Each file is processed within `--interval` seconds:
1. Detected by the watcher
2. Parsed (format-specific extraction)
3. Ingested as an episode with `source_id = "watch:<path>"`
4. Available for `extract` and `assemble_context` queries

No manual `ingest` tool call needed.
</details>

### Optional MCP apps surface

The repository also contains an optional app-oriented MCP surface for reviewer and inspector workflows. It is intentionally feature-gated so the six canonical memory tools stay available without exposing extra session/resource endpoints by default.

Build or run with apps enabled:

```bash
cargo run --features mcp-apps -- serve
```

Recommended verification for this surface:

```bash
cargo check --all-targets --features mcp-apps
cargo clippy --all-targets --features mcp-apps
```

**How it works internally**

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

**Deduplication behavior**

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

The `--interval` flag controls both the poll frequency (how often notify scans the directory) **and** the dedup window (minimum time between ingests of the same file).
</details>

**Command-line reference**

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
The watcher is **fail-fast**: any ingest error terminates the entire watch loop.
The watcher **does not diff content** — every qualifying event triggers a full re-ingest of the file.
`Remove`, `Access`, and `Metadata` change events are ignored.
</details>

**Logging during watch**

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

Run the renderer for the current VS Code schema and copy its JSON into
`.vscode/mcp.json`:

```bash
memory_mcp init --target vscode
```

The generated snippet uses `servers.memory_mcp` with a stdio `command` of
`memory_mcp` and no environment variables. After `cargo build --release` or
`cargo install --path crates/memory-mcp --locked`, the installed binary can be
used directly by the host.

## Configuration

Configuration is loaded from environment variables.

### Storage variables and defaults

| Variable | Type | Default | Required | Description |
| --- | --- | --- | --- | --- |
| `SURREALDB_DB_NAME` | string | `memory` | No | Database name |
| `SURREALDB_NAMESPACES` | comma-separated list | `org` | No | Namespace list |
| `SURREALDB_USERNAME` | string | `root` (embedded); explicit value required (remote) | Remote only | Database username |
| `SURREALDB_PASSWORD` | string | `root` (embedded); explicit value required (remote) | Remote only | Database password |
| `SURREALDB_URL` | URL | unset (embedded) | Remote only | Remote connection URL using `ws`, `wss`, `http`, or `https` |
| `SURREALDB_EMBEDDED` | boolean | inferred from `SURREALDB_URL` | No | Explicit `true`/`false`; remote URLs select remote mode and all other URLs select embedded mode when unset |
| `SURREALDB_DATA_DIR` | path | `$XDG_DATA_HOME/memory_mcp`; else `$HOME/.local/share/memory_mcp`; else `./.memory_mcp` (embedded); unset (remote config) | No | Custom embedded data directory; an existing executable-relative `data/surrealdb` directory may be reused for compatibility, and the effective default root also backs local model caches |
| `SURREALDB_EMBEDDING_DIMENSION` | unsigned integer | unset | No | Existing vector dimension override; the provider fallback is `384` for `local-candle` and `1536` for other embedding providers |

The default local path is embedded RocksDB with no external service, credential,
or model download required to start storage. Remote mode requires a valid URL and
non-empty explicit username and password. NER defaults to the in-process Anno
backend; `zero-config` does not mean the binary has no dependencies.

### Advanced runtime overrides

The following settings are optional for power users. They are read by the same executable used by the no-configuration quick start.

| Variable | Type | Default | Description |
| --- | --- | --- | --- |
| `RUST_LOG` | string | `info` | Logging level; canonical values are `trace`, `debug`, `info`, `warn`, and `error`; `warning` aliases `warn`, and unknown values fall back to `info` |
| `MEMORY_PROMETHEUS_LISTEN_ADDR` | socket address (`IP:port`) | unset | Prometheus HTTP listener address; active only when the `prometheus` feature is compiled and this variable is set |
| `QUERY_LOGGING_ENABLED` | boolean | `false` | Persist `assemble_context` analytics rows into `query_log` when `true` |
| `QUERY_LOG_RETENTION_DAYS` | unsigned integer | `90` | Days to retain persisted `query_log` analytics before best-effort pruning |
| `LIFECYCLE_ENABLED` | boolean | `false` | Enable background lifecycle jobs |
| `LIFECYCLE_DECAY_INTERVAL_SECS` | unsigned integer | `3600` | Decay worker interval in seconds |
| `LIFECYCLE_ARCHIVAL_INTERVAL_SECS` | unsigned integer | `86400` | Archival worker interval in seconds |
| `LIFECYCLE_DECAY_THRESHOLD` | floating-point number | `0.3` | Confidence threshold for fact invalidation |
| `LIFECYCLE_ARCHIVAL_AGE_DAYS` | unsigned integer | `90` | Days before archiving episodes |
| `LIFECYCLE_DECAY_HALF_LIFE_DAYS` | floating-point number | `365` | Half-life in days for decay computation |
| `EMBEDDINGS_ENABLED` | boolean | `false` when unset and no provider is set; `true` when a provider is set and this variable is unset | Enable semantic retrieval; explicit `false` takes precedence over provider selection |
| `EMBEDDINGS_PROVIDER` | string enum | `disabled` when both variables are unset; `local-candle` when `EMBEDDINGS_ENABLED=true` without a provider | Embedding backend: `local-candle`, `openai-compatible`, or `ollama`; when `EMBEDDINGS_ENABLED` is unset, setting a provider enables embeddings, while explicit `false` disables them and explicit `true` enables the selected/default provider |
| `EMBEDDINGS_MODEL` | string | `intfloat/multilingual-e5-small` for `local-candle`; required for external providers when enabled | Model identifier for the selected embedding provider |
| `EMBEDDINGS_MODEL_DIR` | path | unset (derived under the effective data/cache root for `local-candle`) | Optional local cache directory for `local-candle` |
| `EMBEDDINGS_BASE_URL` | URL | unset for `local-candle`; `https://api.openai.com/v1` for `openai-compatible`; `http://127.0.0.1:11434` for `ollama` | Base URL for remote embedding providers |
| `EMBEDDINGS_MAX_TOKENS` | unsigned integer | `384` | Max token budget before `local-candle` chunks long inputs |
| `EMBEDDINGS_TIMEOUT_SECS` | unsigned integer | `15` | Timeout for remote embedding calls |
| `EMBEDDINGS_SIMILARITY_THRESHOLD` | floating-point number | `0.7` | Minimum cosine similarity for semantic matches |
| `EMBEDDINGS_API_KEY` | string | unset | Optional bearer token for OpenAI-compatible providers |
| `NER_PROVIDER` | string enum | `anno` | Entity extraction backend: `anno`, `regex`, or `local-gliner` |
| `NER_MODEL` | string | unset for `anno`/`regex`; `urchade/gliner_multi-v2.1` for `local-gliner` | Hugging Face repository for `local-gliner` |
| `NER_MODEL_DIR` | path | unset (derived under the effective data/cache root for `local-gliner`) | Optional local cache directory for `local-gliner` |
| `NER_LABELS` | comma-separated list | `person`, `company`, `location`, `product`, `event`, `technology` | Runtime labels for `local-gliner` |
| `NER_THRESHOLD` | floating-point number | `0.5` | Confidence threshold for `local-gliner` acceptance |
| `NER_BATCH_SIZE` | positive integer | `1` | Max windows per transformer forward pass; increase only after workload-specific benchmarking |
| `NER_MAX_BATCH_TOKENS` | positive integer | `1536` | Max padded tokens per batch |
| `NER_MAX_CONCURRENCY` | positive integer | `1` | Concurrent local NER inference limit |
| `NER_DEVICE` | string enum | `cpu` | Device for local GLiNER: `cpu`, `metal`, or `auto`; `metal` requires `--features metal`, while `auto` uses Metal when available and otherwise falls back to CPU |
| `GLINER_IDLE_UNLOAD_SECS` | unsigned integer | `0` | Seconds of inactivity before the local GLiNER model is unloaded; `0` keeps it loaded for the process lifetime. After unloading, the first extraction pays the model cold-load latency. |
| `MEMORY_CLAIM_ROLLOUT_STAGE` | string enum | `shadow` | Claim reconciliation rollout stage: `disabled`, `shadow`, `relations`, or `evidence` |
| `MEMORY_CLAIM_CANDIDATE_PAGE_SIZE` | unsigned integer | `256` | Candidate page size for claim reconciliation |
| `MEMORY_CLAIM_INLINE_CANDIDATE_LIMIT` | unsigned integer | `1024` | Inline claim candidate limit |
| `MEMORY_CLAIM_INLINE_BUDGET_MS` | unsigned integer | `50` | Inline claim reconciliation budget in milliseconds |
| `ENTITY_FUZZY_THRESHOLD` | floating-point number | `0.85` | Entity fuzzy-match threshold |

Advanced provider selection may cause network access or model downloads. Keep these variables unset for the local-first quick start.

### Optional build features

The binary supports a few opt-in Cargo features:

| Feature | Effect |
| --- | --- |
| `mimalloc` | Use the mimalloc global allocator instead of the system allocator. This remains an explicit experiment: the fresh macOS matrix reduced physical footprint after GLiNER unload but increased observed RSS to about 2.56 GB, so it is not the server default. Build: `cargo build --release --features mimalloc`. |
| `accelerate` | Enable Candle's Apple Accelerate CPU backend. This is an explicit Apple-specific feature, not a portable package default; the current A/B did not pass the no-degradation gate, so do not present it as a production speedup. Build: `cargo build --release --features accelerate`. |
| `metal` | Enable Candle's Metal backend for explicit macOS GPU experiments. It is not a production default. Build: `cargo build --release --features metal`. |
| `mcp-apps` | Enable the optional interactive MCP app-session surface. It is not required for the eight core tools or the zero-config first-value path. Build: `cargo build --release --features mcp-apps`. |

The allocator evidence is recorded in [`docs/performance/MEMORY_PROFILE.md`](docs/performance/MEMORY_PROFILE.md), the CPU-backend result in [`docs/performance/NER_PERFORMANCE.md`](docs/performance/NER_PERFORMANCE.md), and the policy in [ADR-0034](docs/adr/0034-allocator-and-accelerator-default-policy.md). For infrequent local GLiNER extraction, `GLINER_IDLE_UNLOAD_SECS=30` is the measured workload-specific memory recommendation; the runtime compatibility default remains `0`.

### Scopes and namespaces

Every tool call that reads or writes data uses a **scope** to determine which SurrealDB **namespace** to operate in. The mapping is fixed:

| Scope | Required namespace(s) | Notes |
|-------|----------------------|-------|
| `personal` | `personal` | |
| `team` | `team` or `org` | Falls back to `org` if `team` is not configured |
| `org` | `org` | Default scope for `ingest` and `extract` (inline content) |
| `private-domain` | `private-domain` or `private` | |

**`SURREALDB_NAMESPACES`** is the comma-separated list of SurrealDB namespace names available to the server. Every scope you use in tool calls must resolve to at least one namespace in this list.

**Common mistake:** If `SURREALDB_NAMESPACES=mycompany` and you call `extract` with `scope: "org"`, the lookup fails because namespace `org` is not in the list. Either:

1. Add the required namespace to your config:
   ```
   SURREALDB_NAMESPACES=mycompany,org
   ```
2. Or use a scope that matches a configured namespace (but scopes are fixed — you cannot define custom scopes).

**How `extract` resolves scope:**

- **With `episode_id`**: The server searches **all** configured namespaces to find the episode. The `scope` parameter is ignored — namespace resolution is not needed because the episode already exists in some namespace.
- **With inline `content`**: The `scope` parameter is used (defaults to `"org"`). The content is first ingested into the namespace resolved from that scope, then extracted.

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

At startup the server resolves a **target embedding identity** from the configured provider, model, base URL, and effective dimension.
That identity is persisted per namespace in `embedding_state:fact` as an `active_signature` once the namespace is known to be compatible.
In normal `serve` / `watch` startup, every configured namespace is checked before semantic retrieval is enabled.
If a namespace is already marked `ready` for the same signature, semantic retrieval starts normally.
If a namespace is missing state but is clearly compatible (empty namespace or sampled legacy vectors all match the current dimension), the service bootstraps a `ready` state automatically.
If any namespace is marked `rebuilding`, `failed`, or has embeddings that do not match the configured target, the service **degrades to lexical/graph-only retrieval** instead of mixing incompatible vectors.

That is the safety rail: after a provider switch, normal MCP traffic keeps working, but semantic retrieval is intentionally disabled until embeddings are rebuilt.

#### What happens when you switch providers

To switch, change the environment variables and restart. The server does **not** silently rewrite old vectors during normal startup.

The runtime now separates two modes:

**Normal mode** (`memory_mcp` or `memory_mcp watch ...`) — safe startup checks run first. If stored embeddings are incompatible with the configured target, semantic retrieval is disabled and the process logs `embedding.rebuild_required`.
**Maintenance mode** (`memory_mcp reembed`) — a dedicated one-shot command that forces the configured embedding provider on, rewrites every fact embedding, persists progress, and exits when complete.

This keeps the public MCP tool surface unchanged while giving operators a deterministic recovery path after provider changes.

#### The `reembed` maintenance command

Use the maintenance command after changing any embedding target that should become authoritative for stored facts:

- `EMBEDDINGS_PROVIDER`
- `EMBEDDINGS_MODEL`
- `EMBEDDINGS_BASE_URL`
- effective embedding dimension (including override/probe changes)

Example:

```bash
memory_mcp reembed
```

From the workspace during development:

```bash
cargo run --quiet --bin memory_mcp -- reembed
```

**Flags:**

| Flag | Default | Description |
|------|---------|-------------|
| `--max-failures N` | 10% of total (min 10) | Maximum failed facts before aborting. Use `0` for fail-fast behavior. |
| `--retry-failed` | off | Retry only facts that failed in a previous run. |

What the command does:

1. Resolves the configured target signature and dimension.
2. Loads or creates a persisted control-plane job record at `embedding_job:fact_reembed`.
3. Marks each namespace as `rebuilding` in `embedding_state`.
4. Rewrites **all** fact embeddings, including invalidated / historical facts.
5. Stores fresh metadata on each fact (`embedding_provider`, `embedding_model`, `embedding_dimension`, `embedding_signature`, `embedding_updated_at`).
6. Marks namespaces `ready` on success, or `failed` if the failure quota is exceeded.

The job is **restart-safe** for the same target signature: if the process stops mid-run, invoking `memory_mcp reembed` again resumes from the persisted per-namespace cursor instead of starting from scratch.

**Ctrl+C handling:** Pressing Ctrl+C interrupts the run gracefully. The current fact finishes, job state is persisted with status `interrupted`, and the exit code is 130. Resume with `memory_mcp reembed`.

**Continue-on-error:** By default, the command continues processing after a fact failure. If the number of failures stays within the quota (10% of total, minimum 10), the run completes with status `completed_with_errors`. Use `--max-failures 0` to restore fail-fast behavior. After a run with errors, use `--retry-failed` to retry only the failed facts.

See ADR-0018 for the full architectural rationale.

#### Progress, status, and logging

The maintenance flow supports two modes:

**TTY mode (interactive terminal):** When stderr is a TTY, a live progress bar shows:

```
Reembedding [org] ██████████░░░░░░░░ 1240/3000 (41%) eta 2m 15s | 38/s ✓1230 ✗10
```

- Percentage, processed/total, ETA in human-readable format, facts/sec
- Success/failure counters (`✓1230 ✗10`)
- Namespace label in the bar prefix
- Spinner during service initialization
- Redraw throttled to 10 Hz

After completion, a compact summary is printed to stdout:

```
✓ Reembed completed (with errors)

  Total:       3000 facts
  Processed:   3000 (2990 succeeded, 10 failed)
  Duration:    135.2s
  Speed:       22 facts/sec

  10 facts failed. Re-run with --retry-failed to retry only failures.
```

**Non-TTY mode (pipes, CI, scripts):** When stderr is not a TTY, the command falls back to structured log events:

- `reembed.init_completed` — service initialized, ready to process
- `reembed.namespace_started` / `reembed.namespace_completed`
- `reembed.index_recreating` / `reembed.index_recreated`
- `reembed.progress` — batch-level progress (every 100 facts)
- `reembed.fact_failed` — a fact failed to re-embed (with error reason)
- `reembed.job_interrupted` — Ctrl+C received
- `reembed.job_completed` — final outcome with `outcome` field
- `reembed.job_failed` — quota exceeded or unrecoverable error
- `main.reembed_completed` — compact summary with totals and elapsed time

Job statuses persisted in the control-plane record: `running`, `completed`, `completed_with_errors`, `failed`, `interrupted`.

#### Recommended procedure after switching

To restore semantic retrieval safely after a provider change:

Change the embedding environment variables.
Run `memory_mcp reembed` (or `cargo run --quiet --bin memory_mcp -- reembed`).
Wait for the maintenance run to complete successfully.
Start the normal MCP server again.

Until step 3 completes, the server may intentionally run with semantic retrieval disabled while lexical and graph-based retrieval continue to work.

#### Transient failures from external embedding providers

For external embedding backends (`openai-compatible` and `ollama`), the server now treats transient provider issues differently from hard configuration errors.

Bounded retries with backoff are applied automatically for:

- request timeouts / connect failures
- HTTP `429` rate limits
- retryable upstream statuses such as `408`, `425`, `500`, `502`, `503`, and `504`

If those retries still do not recover the provider:

**write-paths** keep the fact write and schedule an **in-memory background retry** to fill in the missing embedding later;
**query-time semantic retrieval** falls back to lexical / graph-only results for the current request and schedules a background warm-up of a short-lived query embedding cache for repeated identical queries;
**`memory_mcp reembed`** still stops after bounded retries and keeps the maintenance job in a failed state so operators can fix the provider and rerun it explicitly.

Important limitation: the deferred background path is intentionally **in-memory only**. If the process restarts before a background retry succeeds, those deferred retries are lost and will be attempted again only when a new request hits the same path.

#### Similarity threshold

The `EMBEDDINGS_SIMILARITY_THRESHOLD` (default `0.7`) filters semantic search results: only facts with cosine similarity ≥ threshold are returned. After a provider switch, this threshold effectively filters out **all** old facts because cross-provider similarity scores are meaningless.

If you only use **lexical** (BM25/FTS) retrieval and **graph-expanded** context assembly, the provider switch has **no impact** on those retrieval tiers — they do not use embeddings.

### Retrieval behavior

`assemble_context` remains lexical/BM25-first, but now applies deterministic query-mode routing before ranking results:

- explicit `view_mode` still wins;
- temporal-history queries such as `timeline of Atlas changes in Q1 2026` automatically resolve to timeline ordering when `view_mode` is omitted;
- named entity anchors can expand into 1-hop graph context (2 hops for explicit connection/path questions) without requiring semantic retrieval.

### Query analytics logging

Persisted query analytics are **optional** and **disabled by default**.

When `QUERY_LOGGING_ENABLED=true`, successful `assemble_context` calls write a row to the `query_log` table with:

- `scope`
- `query`
- `project`
- `view_mode`
- `resolved_view_mode`
- `query_flags`
- `retrieval_tiers`
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

### Performance benchmarks

Performance measurements live under `crates/eval-harness/benches/` and use
Criterion. They are not part of `cargo test`.

```bash
# Pipeline stages (ingest, extraction, claims, retrieval, end-to-end)
cargo bench -p eval-harness --bench pipeline -- --noplot

# NER on CPU
cargo bench -p eval-harness --bench ner_cpu -- --noplot

# NER on Metal (macOS only; feature belongs to memory_mcp)
cargo bench -p eval-harness --features memory_mcp/metal --bench ner_metal -- --noplot

# NER CPU with Candle Accelerate (macOS only; currently experimental)
cargo bench -p eval-harness --features memory_mcp/accelerate --bench ner_cpu -- --noplot

# Contention
cargo bench -p eval-harness --bench contention -- --noplot
```

See `docs/performance/NER_PERFORMANCE.md` for raw samples, contention results,
and the Criterion reproduction contract.

### MCP Tasks (optional)

The server advertises the official `io.modelcontextprotocol/tasks` extension.
`extract` is the only task-capable tool. A client that advertises the extension
calls `extract` through ordinary `tools/call` and receives a task handle with
`taskId`, `status`, timestamps, TTL, and a suggested polling interval at the
result level. Poll `tasks/get` until the task is terminal; completed payloads are
embedded in the detailed task’s `result` field and failed payloads in its `error`
field. `tasks/update` is available for input responses and `tasks/cancel` requests
cooperative cancellation. Task listing and a separate terminal-result request are
not part of this extension contract.

Clients that do not advertise `io.modelcontextprotocol/tasks` continue to receive
synchronous `extract` results. rmcp’s `TaskManager` supplies the default five-minute
TTL, polling metadata, lifecycle, and retention behavior.

CPU is the production default. Metal remains an experimental, explicit opt-in until its
candidate parity, latency, contention, and memory gates are recorded for the deployment
hardware.

```bash
# Run with Metal GPU (requires --features metal)
NER_DEVICE=metal cargo run --release --features metal -- serve

# Auto tries Metal and falls back to CPU; do not make it a deployment default before gating
NER_DEVICE=auto cargo run --release --features metal -- serve
```

### Binary entry points

- `crates/memory-mcp/src/main.rs` — main MCP server binary

MCP input/output schemas are exposed by the server itself through the protocol's
tool metadata and remain regression-covered by the schema tests under
`crates/memory-mcp/src/mcp/`.

## Testing

Run the production crate's test suite:

```bash
cargo test
```

Run every workspace member, including the private evaluation harness:

```bash
cargo test --workspace
```

Useful narrower runs:

```bash
cargo test --test service_integration
cargo test --test service_acceptance
cargo test --test tools_e2e
```

Coverage output is stored under `coverage/` when generated with Tarpaulin.

## Project layout

```text
.
├── AGENTS.md
├── Cargo.toml              # workspace root
├── Makefile                # thin eval profile adapters
├── crates/
│   ├── memory-mcp/         # production package
│   │   ├── migrations/
│   │   ├── src/            # library, thin binary, MCP and domain services
│   │   └── tests/          # production integration and release-gate tests
│   └── eval-harness/       # private evaluation package
│       ├── benches/        # Criterion benchmark families
│       ├── src/            # domain, artifact, metrics, gate, suites, runner, CLI
│       └── tests/          # harness integration tests and fixtures
├── evals/
│   ├── baselines/          # reviewed comparison artifacts
│   ├── corpora/            # immutable corpus manifests
│   ├── performance/        # pinned-runner config
│   ├── profiles/           # pr.json, release.json, nightly.json
│   └── schema/             # eval-artifact-v1.json
└── docs/
```

## Evaluation

The `eval-harness` crate (`memory-eval` binary) provides a profile-driven,
truthful evaluation system. It is never linked into the production binary.

```bash
# PR profile (target 10 min)
make eval-pr

# Release profile (target 20 min)
make eval-release

# Nightly profile (full end-to-end)
make eval-nightly

# Corpus preparation (one-time, requires network)
cargo run -p eval-harness --bin memory-eval -- prepare-corpus \
  --manifest evals/corpora/longmemeval.json \
  --output-root data/corpora
```

Profiles, modes, outcome semantics, artifact schema, and baseline governance
are documented in the design spec at
`docs/superpowers/specs/2026-07-28-truthful-evaluation-system-design.md`
and the supporting ADRs under `docs/adr/`.

## Documentation

- [`docs/MEMORY_SYSTEM_SPEC.md`](docs/MEMORY_SYSTEM_SPEC.md) — full system specification
- [`docs/superpowers/specs/2026-07-28-truthful-evaluation-system-design.md`](docs/superpowers/specs/2026-07-28-truthful-evaluation-system-design.md) — evaluation architecture and design
- [`docs/SIMPLIFIED_SEARCH_REDESIGN_SPEC.md`](docs/SIMPLIFIED_SEARCH_REDESIGN_SPEC.md) — target-state spec for the upcoming breaking search simplification
- [`docs/ENTITY_RESOLUTION_GUIDE.md`](docs/ENTITY_RESOLUTION_GUIDE.md) — normalization, classification, and alias-resolution reference
- [`docs/GRAPH_RELATION_COMPATIBILITY.md`](docs/GRAPH_RELATION_COMPATIBILITY.md) — relation-table strategy and migration path
- [`docs/INTENT_DRIVEN_MCP_DESIGN_GUIDE.md`](docs/INTENT_DRIVEN_MCP_DESIGN_GUIDE.md) — curated references for intent- and skills-driven MCP design
- [`docs/security-hardening-roadmap.md`](docs/security-hardening-roadmap.md) — current query-surface inventory, deployment assumptions, and remaining hardening work
- [`docs/BACKLOG.md`](docs/BACKLOG.md) — open engineering backlog

## Contributing

This repository follows the conventions in [`AGENTS.md`](AGENTS.md).

In particular:

- keep public APIs stable unless a change is explicitly requested
- avoid introducing dependencies without approval
- prefer typed errors and deterministic behavior
- run formatting, clippy, and tests before considering work done

## CLI Mode

Every memory tool can be invoked directly from the command line. The CLI shares the same implementation as the MCP protocol — zero code duplication.

### Subcommands

| Command | Description |
|---------|-------------|
| `serve` (default) | Run the stdio MCP server |
| `watch <dir>` | Watch a directory and auto-ingest files (requires `cli-watch` feature) |
| `reembed` | Rebuild all fact embeddings after a provider switch. Flags: `--max-failures N`, `--retry-failed` |
| `ingest` | Store raw source material as an episode |
| `extract` | Extract entities, facts, and relationships |
| `resolve` | Resolve entity aliases to a canonical entity id |
| `invalidate` | Invalidate a fact while preserving history |
| `explain` | Get citation-ready source snippets |
| `assemble-context` | Assemble ranked, relevant context for a query |
| `init [--target TARGET]` | Print deterministic, output-only host setup for `vscode`, `claude-desktop`, `codex`, `zed`, or `env` |

`init` is the one authorized output-only onboarding exception to the ordinary
CLI surface. It does not build a service, touch storage, edit files, or change
environment variables.

### Examples

```bash
# Ingest an episode
memory_mcp ingest \
  --source-type email \
  --source-id msg-001 \
  --content "I will finish the API by Friday." \
  --t-ref 2026-06-30T10:00:00Z \
  --scope team

# Extract entities and facts
memory_mcp extract --episode-id episode:abc123

# Extract from inline content
memory_mcp extract \
  --content "Alice works at Acme Corp." \
  --source-type ad-hoc \
  --t-ref 2026-06-30T10:00:00Z \
  --scope team

# Resolve an entity
memory_mcp resolve \
  --entity-type person \
  --canonical-name "Alice Smith" \
  --aliases Alice --aliases "A. Smith"

# Query assembled context
memory_mcp assemble-context \
  --query "What did Alice promise?" \
  --scope org \
  --budget 10

# Invalidate a fact
memory_mcp invalidate \
  --fact-id fact:xyz \
  --reason "Decision reversed" \
  --t-invalid 2026-06-30T00:00:00Z

# Get provenance citations
memory_mcp explain \
  --context-items '[{"content":"API delivery","source_episode":"episode:abc"}]'
```

### Output Format

Memory-operation CLI subcommands print the `ToolResponse<T>` as pretty JSON to **stdout**. The output-only `init` command prints its documented result object to **stdout**:

```json
{
  "status": "success",
  "result": "episode:abc123",
  "guidance": "Call extract next to derive entities and facts.",
  "has_more": false,
  "total_count": 1,
  "next_offset": null
}
```

Structured log events go to **stderr** (controlled by `RUST_LOG`). Successful
CLI results, including `memory_mcp init`, go to **stdout** as JSON. Configuration
failures and other error responses go to **stderr** as JSON:

```json
{
  "error": "Invalid `t_ref` value: bad-date. ...",
  "kind": "Validation",
  "exit_code": 2
}
```

### Exit Codes

| Code | Meaning |
|------|---------|
| 0 | Success |
| 1 | Internal / storage / config error |
| 2 | Validation error or not found |

## License

This project is licensed under the **MIT** license. See [`LICENSE`](LICENSE) for details.

## Agent Memory Lifecycle Integration

`memory_mcp` supports agent-host lifecycle integration through an internal
control plane that does not add new public tools. The eight-tool MCP surface
remains exactly eight tools. The ordinary CLI surface has one separate,
output-only onboarding exception: `memory_mcp init`.

- **Architecture:** A versioned host lifecycle bridge invokes internal
  `LifecycleRecall` and `LifecycleCapture` capabilities, which reuse the
  existing `assemble_context` and inline `extract` paths.
- **Trust:** Derived from the invocation channel, never from public arguments.
  External content cannot become privileged instruction, preference, policy,
  retraction, or procedure.
- **Growth control:** Ignored and duplicate events create zero durable rows.
  Accepted content is stored once. Quotas prevent unbounded ingestion.
- **Procedural memory:** Separately gated and projected through the existing
  `FactType::Experience` seam. Currently shadow-only.

See:
- [ADR 0016](docs/adr/0016-agent-memory-lifecycle-integration.md)
- [Integration Contract](docs/agent_integration/CONTRACT.md)
- [Evaluation Results](docs/evals/AGENT_MEMORY_LIFECYCLE.md)
- [Procedural Memory](docs/evals/PROCEDURAL_MEMORY.md)
