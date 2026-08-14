# Repository Layout

Full directory tree and architecture notes for memory_mcp.

## Directory Tree

```
memory_mcp/
├── Cargo.toml                  # Workspace root (members: "crates/*", default: memory-mcp)
├── crates/
│   ├── memory-mcp/             # Production binary + library crate
│   │   ├── Cargo.toml          # version 1.8.0, edition 2024
│   │   ├── src/
│   │   │   ├── main.rs         # Thin binary entry. Delegates to lib.
│   │   │   ├── lib.rs          # Public API root. Exports MemoryMcp, MemoryService, MemoryError, etc.
│   │   │   ├── runner.rs       # Top-level dispatch: clap parse, build service, route to CLI/MCP.
│   │   │   ├── cli.rs          # CLI module root: Cli enum, commands/, runtime/
│   │   │   ├── mcp.rs          # MCP protocol layer root: handlers/, params/, parsers/, resources/, session/, error.rs, response.rs
│   │   │   ├── service.rs      # Service module root: 17 subdirectories + 37 flat modules
│   │   │   ├── tools.rs        # Protocol-agnostic tool implementations (ingest, extract, resolve, compact, etc.)
│   │   │   ├── config.rs       # Config module root: claims/, embedding/, lifecycle/, ner/, surreal/, constants/, helpers/
│   │   │   ├── storage.rs      # Storage abstraction: client.rs, queries.rs, migrations.rs, narrow stores, helpers/, types/
│   │   │   ├── models.rs       # Models root: access.rs, claim.rs, domain.rs, ids.rs, lifecycle_trace.rs, memory_event.rs, procedure.rs, provenance.rs, request.rs, rounding.rs
│   │   │   ├── logging.rs      # Structured logging
│   │   │   ├── observability.rs# Prometheus metrics (optional, feature-gated)
│   │   │   ├── eval_support.rs # Eval-support API (feature-gated, #[cfg(feature = "eval-support")])
│   │   │   └── service/
│   │   │       ├── core/           # MemoryService builder + helpers
│   │   │       ├── capabilities/   # IngestCapability, ExtractCapability, etc. (the narrow seam)
│   │   │       ├── agent_memory/   # Lifecycle orchestration (capture, recall, policy, worker)
│   │   │       ├── context/        # Multi-tier retrieval pipeline (19 submodules)
│   │   │       ├── episode/        # Episode extraction (fact, entity, communities, triples)
│   │   │       ├── claims/         # Claim reconciliation pipeline
│   │   │       ├── apps/           # MCP app session handlers (feature-gated, mcp-apps)
│   │   │       ├── cache/          # Context cache
│   │   │       ├── embedding/      # Embedding provider implementations
│   │   │       ├── entity_extraction/# Pluggable NER backends (anno, regex, anno-onnx, classic gliner, vago lfm2) behind a registry; shared loaded-model + artifact-store lifecycles live in model_runtime.rs and model_artifacts/
│   │   │       ├── lifecycle/      # Background workers: archival, decay, community rebuild
│   │   │       ├── procedures/     # Procedural memory
│   │   │       ├── query/          # Query utilities
│   │   │       ├── content_extraction/# Content parsing (PDF, HTML, plaintext) + FsWatcher
│   │   │       ├── model_loader/   # ML model loading
│   │   │       ├── model_artifacts/# Artifact store for downloaded model weights
│   │   │   ├── embedding_service.rs, embedding_runtime.rs, error.rs, fact.rs, ingestion.rs,
│   │   │   │   explanation.rs, entity.rs, entity_resolution.rs, scope.rs, startup.rs,
│   │   │   │   conflict_resolver.rs, durable_work.rs, triple_extractor.rs,
│   │   │   │   reembed.rs, reembed_options.rs, reembed_progress.rs, value_helpers.rs,
│   │   │   │   service_context.rs, mock_db.rs (test-only)
│   │   │       └── util/           # ID generation, validation helpers
│   │   │   └── cli/
│   │   │       ├── args.rs, commands.rs, runtime.rs
│   │   │       └── commands/   # One-shot CLI: ingest, extract, resolve, assemble-context, explain, invalidate, init (ADR-0030) + hidden lifecycle subcommands (lifecycle_capture, lifecycle_recall) consumed by hook scripts per ADR-0016 AD-4
│   │   ├── tests/              # 39 integration tests
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
│       │   ├── ner_fixtures.rs # NER benchmark/corpus fixture builders
│       │   ├── error.rs
│       │   ├── test_support.rs
│       │   ├── suites.rs
│       │   ├── corpus.rs
│       │   ├── corpus/
│       │   │   ├── manifest.rs # Corpus manifest parsing
│       │   │   ├── adapters.rs # Corpus adapters
│       │   │   ├── selection.rs# Corpus selection and sampling
│       │   │   └── prepare.rs  # Corpus preparation
│       │   └── suites/         # 14 evaluation suites:
│       │       ├── retrieval.rs, extraction.rs, claims.rs,
│       │       │   end_to_end.rs, lifecycle.rs, capacity.rs,
│       │       │   action_grounding.rs, poisoning.rs,
│       │       │   downstream_qa.rs, external_retrieval.rs,
│       │       │   response_size.rs, ner_quality.rs,
│       │       │   registry.rs, retrieval_cases.rs
│       └── benches/            # Criterion benchmarks
│           ├── pipeline.rs     # Pipeline stage benchmarks
│           ├── ner_cpu.rs      # CPU NER benchmarks
│           ├── ner_metal.rs    # Metal GPU NER benchmarks
│           └── contention.rs   # Contention benchmarks
├── evals/
│   ├── corpora/            # Immutable corpus manifests (SHA-256 verified)
│   ├── longmemeval_v2/     # Prepared LoCoMo/LongMemEval corpora
│   ├── performance/        # Pinned-runner configuration
│   ├── profiles/           # pr.json, release.json, nightly.json, ner_quality.json
│   ├── results/            # Recorded comparison results (e.g. NER)
│   └── schema/             # eval-artifact-v1.json (versioned JSON Schema)
├── docs/                   # Design docs, ADRs, specs, plans
├── hooks/                  # Agent memory capture scripts (stop, precompact) + memory_profile.sh
│   ├── memory_stop_hook.sh
│   ├── memory_precompact_hook.sh
│   ├── memory_profile.sh
│   └── README.md
```

## Key Architecture Notes

- **`main.rs`** is intentionally thin — parses args, selects run mode, delegates to library.
- **`lib.rs`** is the main integration point with all public exports. Exports `eval_support` when `feature = "eval-support"`.
- **`src/service/`** contains core domain logic in 17 subdirectories plus 37 flat modules, many of which are themselves directories.
- **`src/service/capabilities/`** — protocol-agnostic capability modules (IngestCapability, ExtractCapability, etc.) that take `&ServiceContext` (the narrow seam) and delegate to domain services. This deepened structure replaced the `MemoryService` god-object.
- **`src/service/embedding_service.rs`** — `EmbeddingService` struct holding embedding generation, query embedding caching, and background retry logic. Support files in `embedding/` (provider implementations) and `embedding_runtime.rs`.
- **`src/mcp/`** submodules: `handlers` (tool implementations; `apps.rs` there is feature-gated), `params` (parameter structures), `parsers` (validation), `resources` (resource catalog), `session` (app session state), and files `error.rs`, `handlers.rs`, `response.rs`. MCP app sessions live in `handlers/apps.rs`, not a top-level `apps`/`tasks` module.
- **`src/cli/`** — CLI argument parsing, run modes (serve/watch/reembed/init), and hidden internal subcommands (`lifecycle-capture`, `lifecycle-recall`) consumed by hook scripts per ADR-0016 AD-4.
- **`src/tools/`** — Protocol-agnostic tool implementations shared by MCP and CLI. Submodules: `ingest`, `extract`, `resolve`, `assemble_context`, `explain`, `invalidate`, `compact`, plus `params`, `parsers`, `response`, `request_id`.
- **`src/storage/`** — SurrealDB abstraction layer: `client.rs` (select_table, apply_migrations_impl), `queries.rs` (build_* query helpers), `migrations.rs`, narrow stores (`app_store.rs`, `context_store.rs`, `episode_store.rs`, `fact_store.rs`, `reembed_store.rs`), plus `claims.rs`, `procedures.rs`, `agent_memory.rs`, `helpers.rs`, `types.rs`.
- **`src/config/`** — Configuration module with submodules for `surreal`, `embedding`, `ner`, `lifecycle`, `claims`, `constants/`, `helpers/`.

**Feature flags** (all additive, `default = []`): `accelerate` (explicit Apple Accelerate CPU backend), `cli-watch` (filesystem watcher), `mcp-apps` (app session workflows), `prometheus` (Prometheus metrics exporter), `metal` (Apple Metal GPU acceleration for NER), `mimalloc` (optional server allocator), `eval-support` (evaluation support API for harness integration). Neither allocator nor Apple backend is enabled implicitly; see ADR-0034.
