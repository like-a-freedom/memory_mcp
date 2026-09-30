# Architecture Audit Remediation

**Date:** 2026-09-30
**Status:** Accepted direction
**Plan:** `docs/superpowers/plans/2026-09-30-architecture-audit-remediation.md`
**Architecture decision:** ADR-0065 and ADR-0069 are written, in Wave 0. ADR-0066, ADR-0067 and ADR-0068 are written at the point each decision is made, in Waves 3 and 5. All five are named here so the guard in Wave 0 can require an implemented spec to name ADRs that exist.

## Problem

`memory_mcp` has outgrown its shape. The bounded-context reorganisation (ADR-0058) landed, the per-file manifest went with it, and the two checks that would have noticed the consequences were retired in the same breath (ADR-0061, ADR-0063, ADR-0064). What the audit found in the gap is not one defect but a class: **things exist, are believed to be live, and are not.**

Three ways this shows up.

*Nothing reaches them.* `observability/` holds thirty-one recording rules, fifteen alerts and two generated Grafana dashboards, checked by four Python scripts, and no workflow, Makefile target, or Cargo target runs any of it. A rule may read a metric the crate stopped exporting for a release and nobody would find out. The `accelerate` feature has one site, a Cargo feature line, and no build anywhere. The `ner_metal` benchmark compiles under `make bench-check` and is never executed, because the nightly job that would run it is `ubuntu-24.04`. Two specs whose status lines say the opposite of what the code does.

*Nothing needs them.* A compatibility re-export block, four `#[allow(dead_code)]` engine accessors, a config reader whose only caller is its own test, a decay helper whose only callers are its own four tests, two test helpers nothing calls, a store trait method no caller ever asked for, and a 113-line breadth-first graph traversal that the `graph` App's three registered actions cannot reach. Meanwhile eight hand-written `DbClient` fakes duplicate 684 lines of test code that one scriptable mock could carry, and four builders on that mock have no caller.

*Nothing owns it.* Business rules live in the transport adapter that happens to call them: quota admission under `http/`, the Tenant status transition table under `http/`, a 247-line provisioning state machine, Account→Tenant resolution that `CONTEXT.md:52` assigns to `tenancy/api.rs` — so a Tenant Runtime cannot be assembled from the context that owns it. Domain SQL for Fact, Claim, Entity, Community and Episode lives in the storage platform alongside `BI_TEMPORAL_WHERE`, the invariant ADR-0002 exists to protect. Four different callers generate an embedding vector, and two of them route around the truncation limit, the disabled-provider check, and the progress logging that the other two honour. Checkpoint promotion state leaks out through the Entity Extractor seam as a public struct, against `CONTEXT.md:109`'s explicit promise that it does not. And the Local Administrator's own store trait has one adapter, so a test double for it must be as wide as the module it stands in for.

The cost is not tidiness. A guard that was never observed failing is a comment; a `select_table` on thirteen of the twenty-three live tables returns `ConfigInvalid` at runtime rather than a compile error; an operator can put a Tenant in a state the transition table forbids because the handler never consults the table.

## Audit method

The audit read the tree, not the plan that describes it. For every claim it walked the call graph forward from the candidate and backward from every consumer, and it counted. Where a grep disagreed with a read, the read won; six claims in the first draft of the plan were wrong for exactly that reason, and each correction is recorded in the plan's self-review rather than quietly dropped.

Three rules the method followed, and this work keeps:

- **A consumer counts only if it is production code.** A test that exists to exercise a function is not a reason for the function to exist, and is not a reason to call it the live path. `decayed_confidence` and `get_surrealdb_config` fail this test; `memory::retrieval::fact_decayed_confidence`, which is injected as a function pointer so decay is substitutable in tests, passes it.
- **A feature is non-wired until a job builds it.** A Cargo feature line is an intent, not a build.
- **A seam that names another module's type is a leak.** Where the audit found a public signature carrying a type the capability's own contract says it does not expose, that is a finding regardless of how the type is named.

## Candidates

### 1. Dead public surface

- The compatibility re-export block at `src/service.rs:102-124`. It re-exports a long list of names, most of which have no consumer anywhere in the workspace; the rest do reach one, and some of those arrive only through the `service::` path this block exists to provide. The split is a property of the tree, not of this document. The block is a naming convention with no readers, not a compatibility surface. The allowlist an implementer freezes when cutting it must be derived from the tree at execution time, not copied from this document.
- Four `#[allow(dead_code)]` engine accessors, `local_db` (436), `mem_db` (446), `remote_db` (456), `is_local` (467), at `src/storage/client.rs:435-469`, each with a comment naming the work that would use it and no caller.
- `MemoryService::get_surrealdb_config` (`src/service/core.rs:367`), whose only caller is its own test at 927-939.
- `service::query::decayed_confidence` (`src/service/query.rs:17`), called only by its own four tests at 28, 55, 82, 109.
- `make_service_with_query_logging` (72-77) and `seed_fact_with_links_and_project` (198-253) in `tests/common/mod.rs`, suppressed rather than cut because that file compiles per test binary.
- `TaskStore::update_progress_fenced` (`src/http/tasks/state.rs:111`, impl at `worker.rs:316`), zero call sites in production, tests, or the test driver.

### 2. Non-wired functionality

- `observability/` reaches no CI job, Makefile target, or Cargo target. Three of its four checkers import `yaml`; `pyproject.toml` declares `numpy`, `onnxruntime`, and `tokenizers` and no `pyyaml`, so it resolves only by accident through the onnxruntime chain.
- `accelerate` — one site (`crates/memory-mcp/Cargo.toml:62`), zero builds. `mimalloc` — one site (`src/main.rs:1-3`), zero builds. `metal`'s `ner_metal` bench — compiled, never executed.
- Most of the sixteen `Makefile` targets are never invoked by any workflow; four are (`eval-response-size`, `eval-ner-quality`, `bench-check`, `bench-cpu-core`), and a fifth, `bench-cpu`, appears only inside a printed suggestion in the step summary rather than as an invocation.
- `docs/superpowers/specs/2026-09-18-local-admin-auth.md:4` still reads "proposed technical design awaiting review" for a shipped Local Administrator authentication path, and its baseline table still names the removed `control-plane-ui` crate (49, 63, 90, 91) and a `MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE*` variable the code no longer reads. `2026-09-23-path-prefix-deployment.md:4` reads "Approved for planning" for a deployment that is fully implemented and verified in code.

### 3. Container width

`MemoryService` declares 27 `pub(crate)` fields (`src/service/core/builder.rs:30-82`) and 59 methods across four `impl` blocks (`core.rs`, `core/builder.rs`, `reembed.rs`, `apps/graph.rs`). Roughly forty percent of `core.rs` is four store constructors whose bodies are three to eight lines of `X::new(self.db_client.clone(), self.active_namespace.clone())` at lines 34, 44, 53, 80. The composition root is inside the container: `new_from_env_with_mode_and_progress` occupies `src/service/core/builder.rs:196-474`, 279 lines that read config, connect, probe the server version, apply migrations, resolve the embedding startup decision, construct the provider and the NER Backend, and spawn the lifecycle and recovery workers. `bootstrap/` — the module CONTEXT.md already calls privileged wiring — is `cfg(control-plane)` and holds only HTTP integration adapters.

### 4. Policy in the transport adapter

- Quota admission: `enforce_ingest` at `http/registry/plan.rs:149`.
- The Tenant status transition table: `can_transition` at `http/registry/provisioning.rs:78`.
- The provisioning state machine: `provision_one` at `http/leases/migration.rs:236`, running to 482.
- Account→Tenant resolution: `AccountResolver` at `http/registry/account.rs:19`, which `CONTEXT.md:52` assigns to `tenancy/api.rs`.

Both store adapters — `InMemoryStore` and `SurrealRegistryStore` — reach for these rules through the `http/` path, so a business rule and a persistence concern are the same call.

### 5. Domain SQL in the storage platform

`src/storage/queries.rs` builds queries for `fact`, `edge`, `community`, and `episode`, defines `BI_TEMPORAL_WHERE` (line 12) — the visibility predicate ADR-0002 was written for — and switches on thirteen domain tables in `temporal_field_names_for_table` (463). `knowledge_store.rs:46` delegates the knowledge context's own SQL up to the platform, inverting the dependency the seam list is built on. Separately, `validate_table_name` (`src/storage/client.rs:938`) allows ten tables through a hardcoded `ALLOWED_TABLES` (939) while `EXPECTED_SCHEMA_TABLES` (`src/storage/migrations.rs:342`) lists twenty-three, so `select_table` on `claim`, `claim_job`, `claim_key_alias`, `claim_policy`, `claim_relation`, `embedding_job`, `embedding_state`, `entity_extraction_projection`, `event_projection_job`, `memory_capture_audit`, `memory_event`, `procedure_candidate`, or `triple` fails with `ConfigInvalid` at runtime.

### 6. Four embedding-generation paths

`embedding::api::update_canonical_vector` (`src/embedding/api.rs:139`) is the declared single write path, with a named policy for each write reason. Four callers generate vectors. Two go through `EmbeddingService::generate_embedding`. `src/service/fact_orchestration.rs:95` builds its own payload and skips the write policy; `src/service/embedding_recovery.rs:253` calls `provider.embed()` directly, bypassing the truncation limit (`embedding/service.rs:109-129`), the disabled-provider check (`:138`), and the progress logging (`:172-208`). A recovery that silently loses the truncation limit will write a vector for content the other two paths would have refused.

### 7. Checkpoint state leaking through the Entity Extractor seam

A Model Checkpoint's state is the Entity Extractor's business; the fact that it is a Model Checkpoint is not. `ExtractorFingerprint` (`src/knowledge/entity_extraction.rs:156-179`) is a public struct carrying `embedding::model_artifacts::RevisionStatus`, `ValidationStatus`, and `effective_device`, plus repository, revision, and artifact identity. CONTEXT.md:109 states the Entity Extractor "does not expose model architecture, checkpoint format, or runtime details"; this is that, in its public interface, in front of every NER Backend — four construct the rich struct themselves (`unavailable.rs:66`, `gliner.rs:1561`, `lfm2_gliner.rs:431`, `anno_onnx.rs:537`) and the rest inherit the trait's default at `entity_extraction.rs:58`. Thirty-eight model-artifact references sit inside `knowledge/`, including the candidate-versus-known-good promotion decision (`gliner.rs:1668-1709`) and safetensors metadata inference (`:417,447,563`).

### 8. One-adapter store traits

`LocalAdminStore` (`src/service/local_admin/contracts.rs:574`) declares eighteen methods and has exactly one implementation, `SurrealRegistryStore` (`src/http/registry/surreal_store/local_admin.rs:458`). `TaskStore` (`src/http/tasks/state.rs`) declares eleven and has one, `DurableTaskStore` (`worker.rs:159`). A test double for either must be as wide as the production module it stands in for, which is what pushes the retrieval tests toward hand-written fakes rather than the mock that already exists.

### 9. Duplicated logic

Seven pairs, of three kinds. **A module that has no home for something it owns twice:** `normalize_surreal_json` exists in `storage/queries.rs:535` and `storage/helpers.rs:153`, in the same module with identical match arms; three lexical-overlap loops live in `memory/retrieval/lexical.rs` (438, 460, 508) while `shared/search_lexical.rs:3-5` declares itself the single home for exactly that loop. **A type duplicated instead of an algorithm:** `ScoredSpan` (`gliner.rs:111`, private) and `ScoredEntity` (`lfm2_gliner/decode.rs:84`, public) are field-for-field identical, and each has its own `apply_nms` with its own I/O-U threshold. **One name, two meanings:** `PolicyFingerprint::compute_v2` (`models/claim.rs:409`) sorts and hashes; `policy_fingerprint` (`memory/agent_memory/recall.rs:64`) sorts and joins with no hash, and its output is stored verbatim into the Exposure Trace and the recall cache key.

## Decisions

**Wire where a consumer exists; cut where one does not.** Dead surface with a plausible future is still dead. The engine accessors, the config reader, the decay wrapper, the two test helpers, and the unused store-trait method go. Of the eight hand-written `DbClient` fakes in `memory/retrieval.rs`, seven consolidate onto `MockDbClient` and `CommunityLookupDbClient` stays. `MockDbClient` has four builders no test calls (`expect_select_table` 103, `expect_select_table_with` 111, `expect_select_table_panic` 119, `expect_migration_result` 126), and wiring those is cheaper than maintaining 684 lines of fakes. `CommunityLookupDbClient` survives the consolidation because it is the only retrieval test that asserts *how many times* a query shape is issued, which a stateless responder cannot express. Two tests that run against a real in-memory SurrealDB stay for the same reason — they exist to exercise real full-text and bi-temporal visibility.

**The three identical `DbEngine` match arms stay.** `DbEngine::Local` and `DbEngine::Mem` hold `Arc<Surreal<Db>>` while `DbEngine::Remote` holds `Arc<Surreal<Client>>`. The arms are identical in body and not in type; collapsing them needs a `&Surreal<impl Connection>` that Rust cannot return without a boxed trait object, and SurrealDB's `query` requires the concrete `Connection`. `run_query_take` taking `impl Connection` is already what keeps each arm one line.

**The three ranking paths are not duplicates.** `memory/retrieval/ranking.rs:168` fuses results for the assembled context; `memory/retrieval/temporal.rs:589` orders candidates inside a temporal window; `procedures_service/ranking.rs:31` ranks procedure candidates by posterior mean. Three objectives. They are recorded in the code so the next reader does not propose a fourth consolidation.

**`split_whitespace()` at `procedures_service/ranking.rs:72` is not `search_query_terms`.** The procedure ranker wants raw whitespace tokens; `shared::search::search_query_terms` (`search.rs:22`) normalises, lowercases, and filters. Routing one through the other would change which procedure candidates match — a silent retrieval-quality change dressed as a cleanup. The raw split is deliberate and says so.

**`PolicyFingerprint::compute_v2` and the recall helper stay separate.** One is a digest; the other is a readable tag-set key stored in an Exposure Trace. The recall one is renamed for what it is rather than forced through a hash that would make the stored trace unreadable.

**Business policy moves to the owning context, as a pure function.** Quota admission, the Tenant status transition table, and Account resolution become pure functions in the context that owns the subject, called by both store adapters and the router. The consequence is stated rather than hidden: the durable store keeps its quota predicate in a SQL `WHERE` clause (`surreal_store.rs:2482`) because atomicity requires it, and the Rust function runs afterwards on a discarded copy to name the denial (`surreal_store.rs:2512`). The two must not drift, so a test pins them to each other. **ADR-0066** records the decision; the alternative — keeping policy in the store trait — would make every future adapter re-implement it.

**The table allowlist gets one owner per table, and it is a type rather than a list.** Each bounded context lists the tables it owns next to the SQL that touches them. The name reaches storage as a value only a context can build, so a caller outside the context cannot invent one. A test asserts the union equals the live schema exactly, which is the actual gate; the `debug_assert` catches a context reaching for a table it never claimed during development, and a newtype on its own would have been the old problem in a type-safe costume.

**Domain SQL goes back to the domain.** `BI_TEMPORAL_WHERE` moves to `shared/`, because it is a domain invariant three modules import and not a platform detail. The Fact, Entity, Community, and Episode builders move to the contexts that own them. What stays in `storage/` is table-generic: the connection, the create/update/upsert builders, migrations, and the platform's own access and event logs.

**`embedding::api` becomes the only generation interface.** Every vector the system writes goes through one function that cannot skip the truncation limit, the disabled check, or the logging. A test scans the crate for any call to the generator or the provider outside `embedding/` and fails.

**The stdio composition root moves out of the container.** **ADR-0067** records it. The container constructs and holds; `bootstrap/` starts. The rejected alternative is a `service/startup.rs`, because that leaves a second place that knows how to build a service, which is the confusion the move exists to remove.

**Extractors report an opaque revision token.** **ADR-0068** records it. A caller compares tokens for equality and nothing else; the status, the device, and the artifact identity stay in the module that can interpret them. Every NER Backend that builds a fingerprint and every out-of-crate consumer moves together, and the token format is pinned by a test so a change is a deliberate cache invalidation rather than an accident. The rejected alternative is moving the whole trait into `embedding/`, because extraction is a knowledge capability and the backends are not embedding providers.

**The guards come back, and the two decisions that retired them are reversed.** **ADR-0065** records why, so a future review does not propose removing them a second time. The source-tree check exists because the reorganisation left 137 undeclared files and 54,502 lines invisible to rustc while the build stayed green; the doc-claim check exists because eighteen citations to `docs/superpowers/plans/` resolve to nothing today, naming thirteen distinct targets, and the plan this spec accompanies is what makes that directory real. Both are cargo tests, because ADR-0064 requires CI to run nothing outside cargo. **ADR-0069** records the path-prefix deployment, which is fully implemented, has no ADR, and cannot be reconstructed from the code — a reader cannot derive why the cookie name moves from `__Host-` to `__Secure-` or why the base-path sentinel must be stamped at build time.

**No ADR for dead-surface removal, wiring, deduplication, or the storage SQL move.** Each is a consequence of a decision CONTEXT.md and ADR-0058 already state. An ADR for a consequence is a record of nothing.

## Non-goals

- **No new MCP tool.** The eight-tool surface is frozen and `public_surface_snapshot` passes unchanged. The observability work adds a Cargo subcommand, not a tool.
- **No new dependency.** The observability checkers get a declared `pyyaml`, not a new crate; if a task appears to need a dependency, it is misdesigned.
- **No behavioural change in the first wave.** Cutting, wiring, and collapsing duplicates changes what exists and what is reachable, not what the server does. The one deliberate behaviour change in the whole work — an operator transition that the table forbids now answers `409` instead of succeeding — is in the wave that moves the table, and is recorded as a fix.

## Completion

Every candidate closed with a named commit; every guard observed failing before it is believed; five ADRs whose every claim is checkable against the tree; `CONTEXT.md` matching the seams and the vocabulary this work produced; and the working tree clean.
