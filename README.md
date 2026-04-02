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
- **SurrealDB support** for embedded and remote deployments
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

By default, the server starts with:

- embedded storage **disabled** (`SURREALDB_EMBEDDED=false` unless set)
- semantic embeddings **enabled** via the local Candle backend
- default embedding model `intfloat/multilingual-e5-small`
- default NER provider `anno`

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
SURREALDB_DB_NAME=memory \
SURREALDB_NAMESPACES=org,personal \
SURREALDB_USERNAME=root \
SURREALDB_PASSWORD=root \
SURREALDB_EMBEDDED=true \
EMBEDDINGS_ENABLED=false \
RUST_LOG=info \
cargo run --quiet --bin memory_mcp
```

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
                "SURREALDB_DB_NAME": "memory",
                "SURREALDB_NAMESPACES": "org,personal",
                "SURREALDB_USERNAME": "root",
                "SURREALDB_PASSWORD": "root",
                "SURREALDB_EMBEDDED": "true",
                "EMBEDDINGS_ENABLED": "false",
                "RUST_LOG": "info"
            }
        }
    }
}
```

After `cargo build --release` or `cargo install --path .`, you can switch `command` to `./target/release/memory_mcp` or `memory_mcp` respectively.

## Configuration

All configuration is loaded from environment variables at startup.

### Complete environment variables reference

| Variable | Required | Type | Allowed Values | Default | Description |
|----------|----------|------|----------------|---------|-------------|
| **SurrealDB Connection** |||||
| `SURREALDB_DB_NAME` | **Yes** | string | Any valid DB name (e.g., `memory`, `testdb`) | — | Database name |
| `SURREALDB_URL` | Yes when `SURREALDB_EMBEDDED=false` | string | `ws://...`, `wss://...` | — | Remote WebSocket endpoint |
| `SURREALDB_NAMESPACES` | **Yes** | string | Comma-separated list (e.g., `org,personal`) | — | Namespaces to initialize; request `scope` values must match one of these namespaces, except for the explicit `org` alias when no literal `org` namespace is configured |
| `SURREALDB_USERNAME` | **Yes** | string | Any string | — | Database username |
| `SURREALDB_PASSWORD` | **Yes** | string | Any string | — | Database password |
| `SURREALDB_EMBEDDED` | No | boolean | `true`, `false` | `false` | Use embedded RocksDB instead of remote WebSocket |
| `SURREALDB_DATA_DIR` | No | string | Filesystem path | `<exe-dir>/data/surrealdb` | Data directory for embedded mode |
| **Logging** |||||
| `RUST_LOG` | No | string | `trace`, `debug`, `info`, `warn`, `error` | `info` | Logging level |
| **Embeddings** |||||
| `EMBEDDINGS_ENABLED` | No | boolean | `true`, `false` | `true` | Enable semantic embeddings |
| `SURREALDB_EMBEDDING_DIMENSION` | No | integer | `384`, `768`, `1024`, `1536`, `2048`, `3072` | `384` effective default | Vector dimension override |
| `EMBEDDINGS_PROVIDER` | No | string | `local-candle`, `openai-compatible`, `ollama` | `local-candle` | Embedding backend |
| `EMBEDDINGS_MODEL` | Conditional | string | Model or repo ID | `intfloat/multilingual-e5-small` for local-candle | Embedding model name |
| `EMBEDDINGS_BASE_URL` | Conditional | string | URL | provider-specific | Base URL for `openai-compatible`/`ollama` |
| `EMBEDDINGS_API_KEY` | Conditional | string | Bearer token | — | API key for `openai-compatible` providers |
| `EMBEDDINGS_MODEL_DIR` | No | string | Filesystem path | `<data_dir>/models/<model_repo_id>` | Override local embedding model cache directory |
| `EMBEDDINGS_TIMEOUT_SECS` | No | integer | Positive integer | `15` | Request timeout in seconds |
| `EMBEDDINGS_SIMILARITY_THRESHOLD` | No | float | `0.0` – `1.0` | `0.7` | Minimum cosine similarity |
| `EMBEDDINGS_MAX_TOKENS` | No | integer | Positive integer | `384` | Max input tokens for chunking |
| **Lifecycle (Background Jobs)** |||||
| `LIFECYCLE_ENABLED` | No | boolean | `true`, `false` | `false` | Enable background workers |
| `LIFECYCLE_DECAY_INTERVAL_SECS` | No | integer | Positive integer | `3600` | Decay check interval (seconds) |
| `LIFECYCLE_ARCHIVAL_INTERVAL_SECS` | No | integer | Positive integer | `86400` | Archival check interval (seconds) |
| `LIFECYCLE_DECAY_THRESHOLD` | No | float | `0.0` – `1.0` | `0.3` | Confidence threshold for invalidation |
| `LIFECYCLE_ARCHIVAL_AGE_DAYS` | No | integer | Positive integer | `90` | Days before archiving episodes |
| `LIFECYCLE_DECAY_HALF_LIFE_DAYS` | No | float | Positive number | `365.0` | Half-life used by decay workers |
| **NER (Named Entity Recognition)** |||||
| `NER_PROVIDER` | No | string | `regex`, `anno`, `local-gliner` | `anno` | NER backend |
| `NER_MODEL` | No | string | HuggingFace repo ID | `urchade/gliner_multi-v2.1` | GLiNER model (if local-gliner) |
| `NER_MODEL_DIR` | No | string | Filesystem path | `<data_dir>/models/ner/<repo_id_with_slashes_replaced_by_double-dashes>` | Override model cache directory |
| `NER_LABELS` | No | string | Comma-separated | `person,company,location,product,event,technology` | Entity types to extract |
| `NER_THRESHOLD` | No | float | `0.0` – `1.0` | `0.5` | Confidence threshold for entities |
| `NER_BATCH_SIZE` | No | integer | Positive integer | `4` | Texts per inference pass (CPU) |

### Example configuration

#### Minimal embedded setup (development)

```bash
SURREALDB_DB_NAME=memory
SURREALDB_NAMESPACES=org,personal
SURREALDB_USERNAME=root
SURREALDB_PASSWORD=root
SURREALDB_EMBEDDED=true
EMBEDDINGS_ENABLED=false
RUST_LOG=info
```

#### Remote SurrealDB with embeddings (production)

```bash
# Database
SURREALDB_DB_NAME=memory
SURREALDB_URL=ws://127.0.0.1:8000/rpc
SURREALDB_NAMESPACES=org,personal,private
SURREALDB_USERNAME=root
SURREALDB_PASSWORD=root

# Embeddings (OpenAI)
EMBEDDINGS_ENABLED=true
EMBEDDINGS_PROVIDER=openai-compatible
EMBEDDINGS_MODEL=text-embedding-3-small
EMBEDDINGS_BASE_URL=https://api.openai.com/v1
EMBEDDINGS_API_KEY=sk-...

# Lifecycle
LIFECYCLE_ENABLED=true
LIFECYCLE_DECAY_INTERVAL_SECS=3600
LIFECYCLE_ARCHIVAL_INTERVAL_SECS=86400
LIFECYCLE_DECAY_HALF_LIFE_DAYS=365
```

#### Default local embeddings via Candle

```bash
SURREALDB_DB_NAME=memory
SURREALDB_NAMESPACES=org
SURREALDB_USERNAME=root
SURREALDB_PASSWORD=root
SURREALDB_EMBEDDED=true

# Default local embedding backend
EMBEDDINGS_ENABLED=true
EMBEDDINGS_PROVIDER=local-candle
EMBEDDINGS_MODEL=intfloat/multilingual-e5-small
```

#### Ollama embeddings (local)

```bash
SURREALDB_DB_NAME=memory
SURREALDB_NAMESPACES=org
SURREALDB_USERNAME=root
SURREALDB_PASSWORD=root
SURREALDB_EMBEDDED=true

# Ollama embeddings
EMBEDDINGS_ENABLED=true
EMBEDDINGS_PROVIDER=ollama
EMBEDDINGS_MODEL=nomic-embed-text
EMBEDDINGS_BASE_URL=http://127.0.0.1:11434
```

#### Local GLiNER NER (zero-shot entity extraction)

```bash
SURREALDB_DB_NAME=memory
SURREALDB_NAMESPACES=org
SURREALDB_USERNAME=root
SURREALDB_PASSWORD=root
SURREALDB_EMBEDDED=true

# GLiNER NER — downloads model on first run (~200-400MB)
NER_PROVIDER=local-gliner
NER_MODEL=urchade/gliner_multi-v2.1
NER_LABELS=person,company,location,product,event,technology,date
NER_THRESHOLD=0.5
NER_BATCH_SIZE=4
```

The GLiNER provider uses a DeBERTa-v3 model for zero-shot named entity recognition. It runs entirely locally via Candle inference — no external API calls for NER.

An `.env` file already exists in the repository root, so you can keep local values there if your MCP host or shell loads it.

## MCP tools

The public MCP surface is intentionally small and intent-driven. Internal MCP App workflows still exist in the service layer, but the exposed MCP contract is limited to the six canonical memory tools plus one app launcher (`open_app`) and one coarse-grained app mutation bridge (`app_command`). App read-side state is exposed through MCP resources under `ui://memory/app/{app}/{session_id}`.

| Tool | Purpose |
| --- | --- |
| `ingest` | Store an episode with source metadata and timestamps |
| `extract` | Extract entities, facts, and links from an episode or raw content |
| `assemble_context` | Return ranked memory context for a query |
| `explain` | Expand context items with source citations and multi-source provenance |
| `invalidate` | Mark a fact as no longer valid as of a given time |
| `resolve` | Canonicalize an entity name and its aliases into a stable entity identifier |
| `open_app` | Open an app session for inspector, diff, ingestion review, lifecycle, or graph flows and return a session-backed resource URI |
| `app_command` | Execute session-scoped app actions such as ingestion review mutations, lifecycle operations, diff export, graph exploration, or generic session closing |

### App resources

`open_app` returns a `resource_uri` like `ui://memory/app/ingestion_review/{session_id}` plus an immediate JSON fallback in `result.fallback`. Reading the session resource returns `text/html;profile=mcp-app`, which compliant MCP Apps hosts can render inline while still carrying the current session payload in the document. Mutation-heavy app flows use `app_command`, then re-read the resource to observe refreshed state. For graph sessions, `app_command` also exposes session-scoped exploration actions such as `expand_neighbors`, `open_edge_details`, and `use_path_as_context`.

The server also publishes session URI templates via `resources/templates/list`, so MCP hosts that surface resource templates can discover `ui://memory/app/{app}/{session_id}` without guessing the shape of app session URIs.

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
cargo clippy --all-targets -- -D warnings
cargo doc --no-deps
```

Full repository validation:

```bash
cargo fmt --all && cargo check && cargo clippy --all-targets -- -D warnings && cargo test && cargo doc --no-deps
```

### Code Quality & Refactoring

As of 2026-04-01, the refactoring plan is fully implemented and verified in the repository state.

**Completed improvements (13/13 tasks):**

- **DRY violations eliminated**: extracted shared helpers for validation, session handling, lookup, and fact-state derivation
- **SRP compliance**: `commit_ingestion_review()` now orchestrates focused commit helpers, and APP-specific service methods live in `app_modules.rs`
- **API ergonomics**: `add_fact()` now uses `AddFactRequest` instead of a 10-parameter signature
- **Testability**: `MemoryService::new_with_clock()` enables deterministic time-dependent tests, and a shared `MockDb` now backs service unit tests across `core.rs`, `context.rs`, and `episode.rs`
- **Idiomatic Rust**: scope resolution is now strict and deterministic, and the `RateLimiter` mutex usage is explicitly documented with a `SAFETY` note
- **Structural reduction**: `core.rs` dropped from 5,069 to 3,413 lines, with 1,331 APP-focused lines extracted into `app_modules.rs`

**Refactoring details:**

| Task | Change | Impact |
|------|--------|--------|
| Validation | `require_non_empty` helper | Eliminated 8 duplicates |
| Core service | `fact_state` helper | Removed t_invalid duplication |
| Core service | `find_record_in_namespaces` | Deduplicated namespace lookup |
| Core service | `require_app` / `require_target_str` | ~15 session validation duplicates removed |
| Core service | `resolve_entity_by_type` | 6 `resolve_*` methods now delegate |
| Core service | Scope resolution | Exact namespace match or explicit alias only; unknown scopes return a validation error |
| Core service | `commit_ingestion_review` | Split into `commit_entities`, `commit_facts`, `commit_edges`, `finalize_commit`, and draft commit helpers |
| Core service | `AddFactRequest` struct | Reduced from 10 params to 1 struct |
| Service layout | APP module extraction | APP-01..APP-05 moved to `app_modules.rs`; `core.rs` reduced to 3,413 lines |
| Test support | Unified `MockDb` | Shared configurable mock now backs service unit tests in `core.rs`, `context.rs`, and `episode.rs` |
| Test support | Injectable clock | Deterministic time-dependent tests; extracted APP methods use `MemoryService::now()` |

**Quality gates:**
- ✅ `cargo fmt`
- ✅ `cargo check`
- ✅ `cargo clippy -- -D warnings`
- ✅ `cargo test`
- ✅ `cargo doc --no-deps`

Full repository validation for this refactoring pass:

```bash
cargo fmt --all && cargo check && cargo clippy --all-targets -- -D warnings && cargo test && cargo doc --no-deps
```

See [`docs/superpowers/plans/2026-03-31-code-review-refactoring.md`](docs/superpowers/plans/2026-03-31-code-review-refactoring.md) for the complete refactoring plan and status.

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

Verified in this update pass:

- `cargo fmt --all && cargo check && cargo clippy --all-targets -- -D warnings && cargo test && cargo doc --no-deps` → passed

Coverage output is stored under `coverage/` when generated with Tarpaulin.

### Metric eval suites

The repository includes manual metric eval runners that measure retrieval quality, extraction quality, and latency:

#### Built-in eval suites

```bash
cargo test --test eval_extraction -- --ignored --nocapture --test-threads=1
cargo test --test eval_retrieval -- --ignored --nocapture --test-threads=1
cargo test --test eval_latency -- --ignored --nocapture --test-threads=1
```

#### External benchmark eval suites

The repository can convert external memory benchmarks into eval fixtures:

```bash
# Download external datasets first
mkdir -p data/eval_external
curl -sL "https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/main/longmemeval_oracle.json" -o data/eval_external/longmemeval_oracle.json

# Convert to fixtures
python3 scripts/convert_external_evals.py --all

# Run external eval suites
cargo test --test eval_external_retrieval run_longmemeval_retrieval -- --ignored --nocapture --test-threads=1
cargo test --test eval_external_retrieval run_memory_agent_bench_retrieval -- --ignored --nocapture --test-threads=1
```

Supported external sources:
- **LongMemEval** (oracle): 500 questions across 5 memory abilities — Information Extraction, Multi-Session Reasoning, Knowledge Updates, Temporal Reasoning, Abstention
- **MemoryAgentBench**: Accurate Retrieval and Conflict Resolution splits

Important constraints:

- All DB-backed evals run on embedded in-memory SurrealDB only.
- Eval runs must not persist benchmark state or DB artifacts across sessions.
- Normal `cargo test` remains the correctness suite; metric evals are opt-in.

See `docs/superpowers/specs/2026-04-01-mcp-evals-system-design.md` for the eval-system design and `docs/superpowers/plans/2026-04-01-mcp-evals-system.md` for the implementation plan.

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
