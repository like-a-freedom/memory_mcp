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
LOG_LEVEL=info \
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
                "SURREALDB_URL": "rocksdb://./data/surreal.db",
                "SURREALDB_DB_NAME": "memory",
                "SURREALDB_NAMESPACES": "org,personal",
                "SURREALDB_USERNAME": "root",
                "SURREALDB_PASSWORD": "root",
                "LOG_LEVEL": "info"
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
| `SURREALDB_EMBEDDING_DIMENSION` | HNSW vector dimension for embedding indexes (default: `4`; typical production values are `768`, `1024`, or `1536`) |
| `LOG_LEVEL` | Logging level such as `trace`, `debug`, `info`, `warn`, or `error` |

### Example

```bash
SURREALDB_DB_NAME=memory
SURREALDB_NAMESPACES=org,personal
SURREALDB_USERNAME=root
SURREALDB_PASSWORD=root
SURREALDB_URL=ws://127.0.0.1:8000/rpc
SURREALDB_EMBEDDED=false
SURREALDB_EMBEDDING_DIMENSION=1536
LOG_LEVEL=info
```

If you enable a real embedding provider, set `SURREALDB_EMBEDDING_DIMENSION` to the model output size (for example `nomic-embed-text = 768`, `mxbai-embed-large = 1024`, `text-embedding-3-small = 1536`). Changing it for an already-initialized database is **not** automatic: drop and recreate the embedding indexes (or rebuild the DB) before writing vectors with the new dimension.

An `.env` file already exists in the repository root, so you can keep local values there if your MCP host or shell loads it.

## MCP tools

The public MCP surface is centered on a small set of high-value operations rather than endpoint-by-endpoint plumbing.

| Tool | Purpose |
| --- | --- |
| `ingest` | Store an episode with source metadata and timestamps |
| `extract` | Extract entities, facts, and links from an episode or raw content |
| `assemble_context` | Return ranked memory context for a query |
| `explain` | Expand context items with source citations and traceable provenance |
| `invalidate` | Mark a fact as no longer valid as of a given time |

This design lines up with the intent-driven MCP guidance reflected in the docs: fewer tools, clearer semantics, better outcomes.

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
