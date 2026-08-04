# Repository Layout

Full directory tree and architecture notes for memory_mcp.

## Directory Tree

```
memory_mcp/
├── Cargo.toml                  # Workspace root (members: "crates/*", default: memory-mcp)
├── crates/
│   ├── memory-mcp/             # Production binary + library crate
│   │   ├── Cargo.toml          # version 1.7.0, edition 2024
│   │   ├── src/
│   │   │   ├── main.rs         # Thin binary entry. Delegates to lib.
│   │   │   ├── lib.rs          # Public API root. Exports MemoryMcp, MemoryService, MemoryError, etc.
│   │   │   ├── runner.rs       # Top-level dispatch: clap parse, build service, route to CLI/MCP.
│   │   │   ├── cli.rs          # CLI module root: Cli enum, commands/, runtime/
│   │   │   ├── mcp.rs          # MCP protocol layer root: handlers/, params/, parsers/, resources/, session/, tasks/, apps/, error.rs, response.rs
│   │   │   ├── service.rs      # Service module root: 40+ submodules
│   │   │   ├── tools.rs        # Protocol-agnostic tool implementations (ingest, extract, resolve, etc.)
│   │   │   ├── config.rs       # Config module root: claims/, embedding/, lifecycle/, ner/, surreal/, constants/, helpers/
│   │   │   ├── storage.rs      # Storage abstraction: client/, queries/, migrations/, claims/, procedures/, agent_memory/, helpers/, types/
│   │   │   ├── models.rs       # Models root: claim/, domain/, ids/, provenance/, procedure/, access/, lifecycle_trace/, memory_event/, request/
│   │   │   ├── logging.rs      # Structured logging
│   │   │   ├── observability.rs# Prometheus metrics (optional, feature-gated)
│   │   │   ├── eval_support.rs # Eval-support API (feature-gated, #[cfg(feature = "eval-support")])
│   │   │   └── service/
│   │   │       ├── core/           # MemoryService builder + helpers
│   │   │       ├── capabilities/   # IngestCapability, ExtractCapability, etc. (the narrow seam)
│   │   │       ├── agent_memory/   # Lifecycle orchestration (capture, recall, policy, worker)
│   │   │       ├── context/        # Multi-tier retrieval pipeline (18 submodules)
│   │   │       ├── episode/        # Episode extraction (fact, entity, communities, triples)
│   │   │       ├── claims/         # Claim reconciliation pipeline
│   │   │       ├── apps/           # MCP app session handlers (feature-gated, mcp-apps)
│   │   │       ├── cache/          # Context cache
│   │   │       ├── embedding/      # Embedding provider implementations
│   │   │       ├── entity_extraction/# Pluggable NER backends (regex, anno, gliner, llm) behind a registry
│   │   │       ├── lifecycle/      # Background workers: archival, decay, community rebuild
│   │   │       ├── procedures/     # Procedural memory
│   │   │       ├── query/          # Query utilities
│   │   │       ├── content_extraction/# Content parsing (PDF, HTML, plaintext) + FsWatcher
│   │   │       ├── model_loader/   # ML model loading
│   │   │   ├── embedding_service.rs, embedding_runtime.rs, error.rs, fact.rs, ingestion.rs,
│   │   │   │   explanation.rs, entity.rs, entity_resolution.rs, scope.rs, startup.rs,
│   │   │   │   conflict_resolver.rs, durable_work.rs, triple_extractor.rs,
│   │   │   │   reembed.rs, reembed_options.rs, reembed_progress.rs, value_helpers.rs,
│   │   │   │   service_context.rs, mock_db.rs (test-only)
│   │   │       └── util/           # ID generation, validation helpers
│   │   │   └── cli/
│   │   │       ├── args.rs, commands.rs, runtime.rs
│   │   │       └── commands/   # One-shot CLI: ingest, extract, resolve, etc. + hidden lifecycle subcommands (lifecycle_capture, lifecycle_recall) consumed by hook scripts per ADR-0016 AD-4
│   │   ├── tests/              # 29+ integration tests
│   │   │   ├── service_integration.rs, tools_e2e.rs, tools_shared.rs,
│   │   │   │   explain_provenance.rs, embedded_*.rs, apps_*.rs,
│   │   │   │   claim_*.rs, lifecycle_*.rs, procedural_memory_e2e.rs,
│   │   │   │   agent_memory_*.rs, promise_detection.rs, longmem_acceptance.rs,
│   │   │   │   local_model_integration.rs, prometheus_claim_metrics.rs,
│   │   │   │   ingest_select.rs, service_acceptance.rs
│   │   │   └── fixtures/       # Test data
│   │   └── migrations/         # SurrealDB schema migrations (append-only)
│   └── eval-harness/           # Private evaluation harness (not linked into production)
│       ├── Cargo.toml
│       ├── src/
│       │   ├── main.rs         # Binary entry: memory-eval
│       │   ├── lib.rs
│       │   ├── cli.rs          # CLI argument parsing
│       │   ├── domain.rs       # Domain types for evaluation
│       │   ├── artifact.rs     # Evaluation artifact model + serialization
│       │   ├── metrics.rs      # Evaluation metrics computation
│       │   ├── gate.rs         # Release gates (regression budgets)
│       │   ├── runner.rs       # Profile-driven orchestration
│       │   ├── profile.rs      # Profile parsing (pr.json, release.json, nightly.json)
│       │   ├── benchmark.rs    # Benchmark integration
│       │   ├── merge.rs        # Merge/comparison logic
│       │   ├── reducer.rs      # Reduction and aggregation
│       │   ├── report.rs       # Report generation
│       │   ├── adapters.rs     # Private canonical-fact importer
│       │   ├── error.rs
│       │   ├── test_support.rs
│       │   ├── suites.rs
│       │   ├── corpus.rs
│       │   ├── corpus/
│       │   │   ├── manifest.rs # Corpus manifest parsing
│       │   │   ├── adapters.rs # Corpus adapters
│       │   │   ├── selection.rs# Corpus selection and sampling
│       │   │   └── prepare.rs  # Corpus preparation
│       │   └── suites/         # 10 evaluation suites:
│       │       ├── retrieval.rs, extraction.rs, claims.rs,
│       │       │   end_to_end.rs, lifecycle.rs, capacity.rs,
│       │       │   action_grounding.rs, poisoning.rs,
│       │       │   downstream_qa.rs, external_retrieval.rs
│       └── benches/            # Criterion benchmarks
│           ├── pipeline.rs     # Pipeline stage benchmarks
│           ├── ner_cpu.rs      # CPU NER benchmarks
│           ├── ner_metal.rs    # Metal GPU NER benchmarks
│           └── contention.rs   # Contention benchmarks
├── evals/
│   ├── baselines/          # Reviewed comparison artifacts for regression budgets
│   ├── corpora/            # Immutable corpus manifests (SHA-256 verified)
│   ├── longmemeval_v2/     # LongMemEval-V2 corpus
│   ├── performance/        # Pinned-runner configuration
│   ├── profiles/           # pr.json, release.json, nightly.json
│   └── schema/             # eval-artifact-v1.json (versioned JSON Schema)
├── docs/                   # Design docs, ADRs, specs, plans
├── hooks/                  # Agent memory capture scripts (stop, precompact)
│   ├── memory_stop_hook.sh
│   ├── memory_precompact_hook.sh
│   └── README.md
```

## Key Architecture Notes

- **`main.rs`** is intentionally thin — parses args, selects run mode, delegates to library.
- **`lib.rs`** is the main integration point with all public exports. Exports `eval_support` when `feature = "eval-support"`.
- **`src/service/`** contains core domain logic in 40+ submodules, many of which are themselves directories.
- **`src/service/capabilities/`** — protocol-agnostic capability modules (IngestCapability, ExtractCapability, etc.) that take `&ServiceContext` (the narrow seam) and delegate to domain services. This deepened structure replaced the `MemoryService` god-object.
- **`src/service/embedding_service.rs`** — `EmbeddingService` struct holding embedding generation, query embedding caching, and background retry logic. Support files in `embedding/` (provider implementations) and `embedding_runtime.rs`.
- **`src/mcp/`** submodules: `apps` (feature-gated), `handlers` (tool implementations), `params` (parameter structures), `parsers` (validation), `resources` (resource catalog), `session` (app session state), `response`, `tasks` (async task orchestration), `error` (error conversion).
- **`src/cli/`** — CLI argument parsing, run modes (serve/watch/reembed), and hidden internal subcommands (`lifecycle-capture`, `lifecycle-recall`) consumed by hook scripts per ADR-0016 AD-4.
- **`src/tools/`** — Protocol-agnostic tool implementations shared by MCP and CLI. Submodules: `ingest`, `extract`, `resolve`, `assemble_context`, `explain`, `invalidate`, plus `params`, `parsers`, `response`, `request_id`.
- **`src/storage/`** — SurrealDB abstraction layer with `client/`, `queries/`, `migrations/`, `claims/`, `procedures/`, `agent_memory/`, `helpers/`, `types/`.
- **`src/config/`** — Configuration module with submodules for `surreal`, `embedding`, `ner`, `lifecycle`, `claims`, `constants/`, `helpers/`.

**Feature flags** (all additive, `default = []`): `cli-watch` (filesystem watcher), `mcp-apps` (app session workflows), `prometheus` (Prometheus metrics exporter), `metal` (Apple Metal GPU acceleration for NER), `eval-support` (evaluation support API for harness integration).
