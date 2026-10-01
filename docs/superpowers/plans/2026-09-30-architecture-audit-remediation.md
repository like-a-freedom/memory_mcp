# Architecture Audit Remediation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Close all nine deepening candidates from the architecture audit, reinstate the two retired guards, and leave no non-wired or dangling functionality.

**Architecture:** Six waves. Wave 0 reinstates the guards so every later wave is verified. Wave 1 is behaviour-preserving: cut dead surface, wire what has a consumer, collapse duplicates. Wave 2 makes `embedding::api` the single generation path. Wave 3 moves four business policies out of the HTTP transport adapter into the owning bounded context's `api.rs` as pure functions. Wave 4 returns domain SQL and the table allowlist to their owners. Wave 5 moves the stdio composition root to `bootstrap/` and narrows the Entity Extractor interface. Every wave is one mergeable commit range and ends green.

**Tech Stack:** Rust 1.97.1, Cargo workspace (`memory-mcp`, `eval-harness`, `ui`, `xtask`), SurrealDB 3.2.4, Tokio, Axum 0.8.9, axum `Router::nest`, `async_trait`, `thiserror`, `metrics` + `metrics-exporter-prometheus`.

**Spec:** `docs/superpowers/specs/2026-09-30-architecture-audit-remediation.md` — written as the first step of Task 0.1. This plan is the plan; the spec records the problem statement and the nine candidates so the spec and the plan agree on vocabulary. ADRs 0065–0069 are written in Wave 0 and Wave 5.

**Commit location:** at execution time, commit this file as `docs/superpowers/plans/2026-09-30-architecture-audit-remediation.md`. This also repairs the 19 dangling `docs/superpowers/plans/` citations (see Task 1.4).

---

## Global Constraints

These apply to every task. A task's requirements implicitly include this section.

- `cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings` must produce zero warnings.
- `cargo fmt --all --check` must produce zero diff.
- `cargo test --workspace --lib --bins --tests --locked` must pass.
- Production code uses `MemoryError` and `Result`. No production `unwrap`, `expect`, or `panic`. (Verified: `http/` and `control/` currently have zero; keep it that way.)
- No lock guard lives across `.await`.
- Metrics labels use bounded enums only. Every metric name is a constant in the vocabulary module, never a string literal at a call site.
- No `#[allow(dead_code)]` may be added. Wave 1 removes the four that exist.
- No `#[allow(dead_code)]` on `pub` items in `tests/common/` — that file is compiled per test binary, so unused helpers there must be cut, not suppressed.
- Migration files under `crates/memory-mcp/migrations/` are append-only. `EXPECTED_SCHEMA_*` in `storage/migrations.rs` is a live startup assertion, not a migration, and may be reorganised.
- The eight-tool MCP surface is frozen. No task adds a tool. `tests/agent_memory_lifecycle_release_gate.rs::public_surface_snapshot` must pass unchanged.
- `MemoryService` may be named only in `service/memory_container_shims/`, `service/capability_deps.rs`, `service/retrieval_deps_from_container.rs`, `service/core/`, `service/cli/`, `runner.rs`, `mcp/`, `cli/`, `tools/`, `http/runtime/`, and the entry points that construct one. No bounded context names it.
- Do not add a dependency. If a task appears to need one, it is misdesigned.
- Rust lints the project denies: read `crates/memory-mcp/src/lib.rs` for the `#![deny(...)]` list before writing code.

## Definition of Done — a task is done only when all seven hold

1. **Tests written first and observed failing.** The plan names the test and the expected failure message. A test never written red is not evidence.
2. **Every named test passes**, plus the full suite in Global Constraints.
3. **Clippy and fmt clean** with the exact commands above.
4. **No new suppression.** `grep -rn "#\[allow(dead_code\|allow(unused" crates/*/src` returns no line not already in the baseline list at Task 1.1.
5. **The wave's review checklist passes** (below).
6. **CONTEXT.md updated** if the task changed a named concept, a module seam, or a constraint. CONTEXT.md is a glossary: no implementation detail, no file paths that are not seams.
7. **Commit message follows the repo's convention**: `type(scope): what changed and why`, where `type` ∈ {feat, fix, refactor, docs, test, chore}. Past commits use lowercase imperative subjects, e.g. `refactor(observability): split the vocabulary from the recording, as ADR-0058 asks`.

## Review checklist — every task review runs this

The reviewer reads the diff and answers each question with evidence or a verdict. A task is not approved with an unanswered question.

- [ ] **TDD**: can I see, in the commit history or the diff, the test failing before the implementation? Name the step.
- [ ] **Depth**: does the interface shrink or stay flat? A change that only adds a function to a module is not a deepening.
- [ ] **Seam discipline**: does anything cross a seam that it did not cross before? Name it.
- [ ] **Two-adapter test**: is every new trait justified by two adapters, or is it hypothetical?
- [ ] **DRY**: does this introduce a second copy of anything that already exists? Search the crate for the name.
- [ ] **KISS**: is there a simpler shape that satisfies the same test? If so, name it and say why it was rejected.
- [ ] **YAGNI**: is any parameter, field, or branch not exercised by a test or a named caller?
- [ ] **Interface is the test surface**: does any test reach past the interface into a private method? The existing `reembed.rs:1899,1904,2299,2180` do; new code must not.
- [ ] **Error path**: does the new code have a `Result`/`MemoryError` path, and is it tested?
- [ ] **Production purity**: zero `unwrap`/`expect`/`panic` outside `#[cfg(test)]`.

## Wave review checklist — every wave boundary runs this

A fresh reviewer who did not see the tasks reads the whole wave's diff range. Per-task reviews cannot see a violation that spans two tasks.

- [ ] **Cross-task DRY**: did two tasks in this wave solve the same problem in two places?
- [ ] **Coherent interface**: do the names and types introduced across tasks agree? A type renamed in task 3 and used under the old name in task 5 is the failure this catches.
- [ ] **Wave DoD** (below) is fully met.
- [ ] **No scope creep**: nothing in the wave that its task did not name.
- [ ] **The wave is independently mergeable**: `git log` shows no commit that fails the Global Constraints on its own.

## Wave DoD

| Wave | DoD |
|---|---|
| 0 | Both guard tests exist, fail when their invariant is broken (proved by a deliberate temporary break), and pass. ADR-0065 committed. Spec committed. |
| 1 | Zero items on the dead-surface list remain. Every wired item has a named consumer and a test that exercises it. Duplicate pairs collapsed. Spec status lines match the code. `plans/` citations resolve. |
| 2 | Every vector write goes through `embedding::api`. No caller reaches the provider directly. |
| 3 | All four policies live in their owning `api.rs` as pure functions. Both store adapters call the context function, not a local copy. No policy test needs an embedded engine. |
| 4 | `storage/` names no domain table and owns no domain SQL. The allowlist has one owner per table and no gap against the live schema. |
| 5 | `new_from_env_with_mode_and_progress` is not in `service/`. The Entity Extractor interface names no model-architecture type. |

---

## ADR allocation

Five ADRs, at the point the decision is made rather than up front. Per domain-modeling: an ADR is written only when a decision is hard to reverse, surprising without context, and the result of a real trade-off.

| ADR | Written in | Decision | Why it earns a record |
|---|---|---|---|
| **0065** | Wave 0 | Reinstate the source-tree and doc-claim guards as cargo tests | Reverses three accepted ADRs (0061, 0063, 0064). A future reviewer will otherwise re-propose removing them again. |
| **0066** | Wave 3, at Task 3.1 | Business policy lives in the owning context, not the store adapter | Not free: it means the store adapters can no longer be self-contained, and the quota predicate must stay in SQL for atomicity. A reader will wonder why the rule is in two places. |
| **0067** | Wave 5, at Task 5.1 | The stdio composition root moves to `bootstrap/` | Changes where every reader looks for startup. `bootstrap/` is currently `cfg(control-plane)` and HTTP-only; making it the stdio composition root is a structural choice with an alternative (keep a `service/startup.rs`). |
| **0068** | Wave 5, at Task 5.6 | Extractors report an opaque revision token | Changes a public trait used by seven adapters and two tests outside the crate. Irreversible once third parties depend on the fingerprint shape. |
| **0069** | Wave 0, at Task 0.3 step 4 | The path-prefix deployment and its build-time base-path sentinel | Found during this plan's review: the feature is fully implemented and has **no** ADR. It is hard to reverse once hosts depend on a base path, and a reader cannot reconstruct why the cookie name moves from `__Host-` to `__Secure-` from the code. |

No ADR for: dead-surface removal, wiring, deduplication, or the storage SQL move. Each is a consequence of an accepted decision, and CONTEXT.md plus ADR-0058 already state the target.

---

## Review Focus

The five input classes or conditions the spec implies that no existing test exercises, most likely to bite first. Each line names the test added to the task that owns the code.

1. **A `select_table` call on any of the 13 tables missing from `ALLOWED_TABLES`** — `claim`, `claim_job`, `claim_key_alias`, `claim_policy`, `claim_relation`, `embedding_job`, `embedding_state`, `entity_extraction_projection`, `event_projection_job`, `memory_capture_audit`, `memory_event`, `procedure_candidate`, `triple`. Today this returns `ConfigInvalid`. A caller that adds a new table and forgets the allowlist gets a confusing config error at runtime, not a compile error. **Test:** `every_expected_schema_table_is_selectable` in `crates/memory-mcp/tests/typed_record_accessors.rs` (Task 4.1).
2. **Two of the three control-plane operator transitions can reach a state the transition table forbids.** `control/operator.rs` calls `store.update_tenant_state` directly, so `can_transition` is never consulted. `suspend_tenant` (138-175) rejects only `Deleting`/`Purged` and will therefore accept `Reserved -> Suspended`, `NamespaceCreating -> Suspended` and `Migrating -> Suspended`, none of which appear in the table. `resume_tenant` (177-197) passes `Suspended -> Ready` but never checks the tenant is actually `Suspended`: the store's compare-and-set is on `expected_version`, not on `from`, so resuming a `Migrating` tenant issues `Migrating -> Ready` and the provisioning loop and the operator both believe the tenant is ready. `retry_tenant` (110-135) is already correct — it gates on `status == Failed` and uses `retry_stage`, which is a legal transition. **Test:** `operator_transitions_obey_the_transition_table` in `crates/memory-mcp/tests/http_control_plane.rs` (Task 3.4) — covers `Migrating -> Suspended` refused and `Migrating -> Ready` refused, and asserts `retry_tenant` still works so the fix is not over-broad.
3. **Quota admission under contention on the durable store.** `surreal_store.rs:2482` enforces the quota in a SQL `WHERE` clause; `enforce_ingest` at `:2512` runs afterwards on a discarded local copy to produce the denial reason. Two requests racing past the Rust check both rely on the SQL. After the policy move, the two must not drift. **Test:** `quota_predicate_matches_the_context_policy` in `crates/memory-mcp/tests/http_registry_storage.rs` (Task 3.3) — asserts every reason string `enforce_ingest` can return is reachable through the durable store.
4. **An `Edge` created through a path that does not set provenance.** `service/apps/graph.rs:79-82` hardcodes `EdgeOrigin::Inferred`, `strength: 1.0`, `confidence: 0.8`, `Provenance::manual()`. `KnowledgeGraphStore::relate_edge` is the production path and takes them as parameters. A caller reaching the former gets a manual-provenance edge at fixed confidence with no way to say otherwise. **Test:** `relate_records_an_operator_originated_edge` in `crates/memory-mcp/tests/knowledge_read_scopes.rs` (Task 5.3, not 5.7 — the first draft of this plan cited a task number that did not exist).
5. **A `plan.rs` `reconcile_usage` name collision.** The pure function `reconcile_usage` (`plan.rs:214`) and the `UsageStore` trait method `reconcile_usage` (`storage.rs:441`) share a name. Moving the pure function into `operations/quota.rs` puts it in a different module, so a `use` of both is legal but a reader will assume they are the same thing. **Test:** none — this is a naming hazard. Prevented by naming the moved function `usage_drift_report`, and asserted in the Task 3.1 review checklist rather than 3.2.

---

# Wave 0 — Guards

Nothing else can be verified until the tree and the documentation have a check. Two facts are true today and nothing enforces either.

## Task 0.1: Write the spec and the plan file

**Files:**
- Create: `docs/superpowers/specs/2026-09-30-architecture-audit-remediation.md`
- Create: `docs/superpowers/plans/2026-09-30-architecture-audit-remediation.md` (this file)

**Interfaces:**
- Consumes: nothing.
- Produces: the spec that the doc-claim guard in Task 0.3 checks, and the `plans/` directory whose existence ADR-0064's audit previously required.

The spec records: the nine candidates, the audit method, the decision to wire rather than delete where a consumer exists, and the five ADRs. It is a problem statement, not a second copy of this plan.

- [x] **Step 1: Write the spec** with sections `Problem`, `Audit method`, `Candidates` (nine, each with the evidence and the file:line list from this plan), `Decisions` (the rulings: cut the 32 re-exports, cut the 4 engine accessors but leave the engine dispatch matches alone, cut `get_surrealdb_config`, cut 2 test helpers, wire `tenant_of`, cut `update_progress_fenced`, move the allowlist to the owning contexts, consolidate the retrieval fakes onto `MockDbClient`, do not narrow the store traits beyond what the policy move requires, cut the unreachable introduction chain), `Non-goals` (no new MCP tool, no new dependency, no behavioural change in Wave 1).
- [x] **Step 2: Commit the spec and this plan together**

```bash
git add docs/superpowers/specs/2026-09-30-architecture-audit-remediation.md \
        docs/superpowers/plans/2026-09-30-architecture-audit-remediation.md
git commit -m "docs: record the architecture audit and the remediation plan"
```

**DoD:** both files committed; `ls docs/superpowers/plans/` returns the plan.

## Task 0.2: Reinstate the undeclared-source guard as a cargo test

ADR-0063 retired this check and ADR-0061 recorded that the compiler graph is the source of truth. Nothing derives the fact. The audit verified 384 `.rs` files and 0 orphans — this test makes that permanent. Reverses ADR-0061/0063/0064, so ADR-0065 records why.

**Files:**
- Create: `crates/memory-mcp/tests/source_tree_integrity.rs`
- Modify: `docs/adr/0061-compiler-graph-defines-the-source-tree.md` (add a superseded-in-part note)
- Modify: `docs/adr/0063-retire-the-undeclared-source-audit.md` (add a superseded-in-part note)

**Interfaces:**
- Consumes: `CARGO_MANIFEST_DIR` (resolves to `crates/memory-mcp`).
- Produces: test fn `every_source_file_is_reachable_from_a_mod_declaration`.

The test walks `crates/memory-mcp/src` recursively for `.rs` files, then reads every source file in the crate and extracts module declarations: `mod x;`, `pub mod x;`, `pub(crate) mod x;`, and `#[path = "..."] mod x;`. A file is reachable if some declaration's resolved path is it. A declaration whose body is `{` is an inline module, not a file reference, and is skipped — this is the case that produces false positives today (`embedding/model_artifacts.rs:35` `pub mod runtime { … }` has no file on disk).

- [x] **Step 1: Write the failing test**

```rust
#[test]
fn every_source_file_is_reachable_from_a_mod_declaration() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut declared: HashSet<PathBuf> = HashSet::new();
    for file in walk(&src) {
        let text = fs::read_to_string(&file).expect("readable source");
        for decl in module_declarations(&text) {
            declared.insert(resolve(&file, &decl));
        }
    }
    let orphans: Vec<PathBuf> = walk(&src)
        .filter(|f| f.extension() == Some("rs") && !declared.contains(f))
        .collect();
    assert!(orphans.is_empty(), "no `mod` declaration reaches: {orphans:#?}");
}
```

`module_declarations(text) -> Vec<Decl { is_path: bool, name: String }>` skips any `mod` line whose following non-comment token is `{`. `resolve` maps a name to `parent_dir/name.rs` and, when the parent is a file stem like `foo.rs`, to `foo/name.rs`, matching Rust 2018 module layout. Write the helpers as private functions in the test file.

- [x] **Step 2: Run it to verify it passes** (the tree is clean today — this is the one guard whose failing state we must manufacture)

Run: `cargo test -p memory_mcp --test source_tree_integrity`
Expected: PASS. A failure here means the tree already has an orphan; stop and report it.

- [x] **Step 3: Prove the test can fail** — create `crates/memory-mcp/src/never_declared.rs` containing `pub fn probe() -> i32 { 0 }`, do not declare it.

Run: `cargo test -p memory_mcp --test source_tree_integrity`
Expected: FAIL with the orphan path in the message. This is the proof the guard works; a guard never observed failing is not a guard.

- [x] **Step 4: Remove the probe file** and re-run.

Expected: PASS.

- [x] **Step 5: Commit the test**

```bash
git add crates/memory-mcp/tests/source_tree_integrity.rs
git commit -m "test: guard the source tree against undeclared files, as ADR-0061 intended"
```

## Task 0.3: Reinstate the doc-claim guard as a cargo test

ADR-0064 removed this because `docs/superpowers/plans` was empty and the audit exited. Task 0.1 makes the directory non-empty, so the guard has something to check. The audit found 19 citations, 20 distinct targets.

**Files:**
- Create: `crates/memory-mcp/tests/doc_claims.rs`
- Modify: `docs/adr/0064-run-ci-on-cargo-only.md` (add a superseded-in-part note)

**Interfaces:**
- Consumes: `CARGO_MANIFEST_DIR/../..` = the workspace root.
- Produces: three test fns: `every_relative_markdown_link_resolves`, `every_spec_status_line_matches_the_code`, `every_adr_cites_an_existing_file`.

- [x] **Step 1: Write the failing test** `every_relative_markdown_link_resolves`: for every `*.md` under `docs/` and `AGENTS.md`/`README.md`/`CONTEXT.md` at the root, extract markdown links `[text](target)`; skip `http://`, `https://`, `#`, and `mailto:`; for the rest, resolve relative to the file's directory and assert the path exists.

Run: `cargo test -p memory_mcp --test doc_claims every_relative_markdown_link_resolves`
Expected: FAIL, listing the 19 `plans/` citations. If it passes, the extraction is wrong — check the regex against a known-dangling line, `docs/adr/0058-bounded-contexts-modular-monolith.md:7`.

- [x] **Step 2: Repoint the citations.** Grouped by what is honest:

  *Point at the spec, where the spec is the surviving record* — `docs/adr/0016:28`, `0017:7`, `0036:5`, `0051:5`, `0055:54` (both targets), `docs/superpowers/specs/2026-09-18-local-admin-auth.md:5`, `docs/superpowers/specs/2026-09-23-path-prefix-deployment.md:9-10`, `crates/memory-mcp/src/memory/procedures_service.rs:8`. Replace the `plans/` target with the `specs/` sibling and adjust the surrounding words from "plan" to "spec" where the sentence refers to a design record.

  *Point at the ADR that superseded the plan* — `docs/adr/0039:6`, `0040:6` (both cite `2026-08-19-architecture-deepening.md`; the decisions they record are the ADRs themselves, so the link is redundant — delete the line and keep the ADR text). `docs/adr/0058:7` and `0059:23` cite the DDD plan; replace with a pointer to this spec.

  *Delete as stale measurement references* — `Cargo.toml:32` (`2026-08-27-streamable-http-saas.deps.md`), `docs/adr/0034:144` (`2026-08-06-allocator-accelerator-defaults.md`), `docs/performance/MEMORY_PROFILE.md:75` (a `.txt` baseline), `crates/memory-mcp/tests/embedded_fts_search.rs:395` (`2026-06-29-plan-review-critical-analysis.md`). These record a moment, not a design. Where the number still matters, the doc keeps the number and loses the link.

  *Reword the two directory references* — `AGENTS.md:126` (keep the entry, point at `docs/superpowers/plans/`, which now exists) and `docs/adr/0064:38` (historical prose about why an audit exited; leave the sentence, it is a true record of a past event, but the guard must not treat prose inside backticks as a link — verify the extractor only matches `](...)`).

- [x] **Step 3: Run the test.** Expected: PASS.

- [x] **Step 4: Write `every_spec_status_line_matches_the_code`.** For each of the 8 files in `docs/superpowers/specs/`, read the `**Status:**` line and assert one of three exact values: `Implemented`, `Accepted direction`, `Superseded`. Maintain the expected value in a table in the test keyed by filename. Then write the second assertion, which is the one that catches drift: a spec declaring `Implemented` must name at least one ADR in its `**Architecture decision:**` line or its body, and that ADR file must exist. Update the two inverted specs:

  * `2026-09-18-local-admin-auth.md:4` — status becomes `Implemented`, with the shipping ADRs named. Also correct lines 49, 63, 90, 91, which reference the removed `control-plane-ui` crate and the removed `MEMORY_MCP_HTTP_ENABLE_CONTROL_PLANE*` variables. `docs/adr/0055-local-admin-authentication-for-remote-deployment.md` and `0057-additive-browser-auth-methods.md` are the shipping decisions.
  * `2026-09-23-path-prefix-deployment.md:4` — status becomes `Implemented`. The four decisions are verified in code: `http/router.rs:346` `Router::nest`, `http/config/parse.rs:215,277`, `ui/assets.rs:49-909` (12 tests), `control/session.rs:82-253` and `control/local_admin/csrf.rs:36-359`.

    **No ADR records the path-prefix deployment** — `grep -rln "path.prefix\|base_path\|BASE_PATH" docs/adr/` returns nothing, and neither ADR-0052 nor ADR-0058 covers it. Two options, and the plan takes the second: (a) leave the spec as `Accepted direction` and note the implementation has no ADR, which is a permanent untidy loose end; (b) write ADR-0069 recording the path-prefix deployment. Choose (b) — it is a real decision (where the base path comes from, why `BASE_PATH_SENTINEL` must be stamped at build time, why the cookie name moves from `__Host-` to `__Secure-`), it is hard to reverse once hosts depend on it, and a future reader genuinely cannot reconstruct it from the code. Write it in this step. Update the ADR allocation table below to list five ADRs, not four.

  Run: `cargo test -p memory_mcp --test doc_claims every_spec_status_line_matches_the_code`
  Expected: FAIL before the status edits (both files inverted), PASS after.

- [x] **Step 5: Write `every_adr_cites_an_existing_file`.** For each of the 64 ADRs, extract markdown links and `docs/...` path references in backticks; assert each resolves. Run: expect PASS, or fix what it finds.

- [x] **Step 6: Commit**

```bash
git add crates/memory-mcp/tests/doc_claims.rs docs/ Cargo.toml AGENTS.md
git commit -m "fix(docs): repoint the plan citations, and guard them, as ADR-0064 intended"
```

## Task 0.4: Record ADR-0065

**Files:**
- Create: `docs/adr/0065-reinstate-the-source-tree-and-doc-claim-guards.md`

**Interfaces:**
- Consumes: the three superseding-in-part notes from Tasks 0.2 and 0.3.
- Produces: the record a future architecture review needs so it does not re-propose removing these tests.

Use the ADR-FORMAT sections: Status, Context, Decision, Consequences, Alternatives considered. The Context records the concrete failure that motivated the guard: the bounded-context migration left 137 undeclared files and 54,502 lines invisible to rustc while the build stayed green (ADR-0061:35). The Decision records that both checks are cargo tests because ADR-0064 requires CI to run nothing outside cargo, and that a test is the only shape that survives that constraint. Consequences: two tests that must be updated when the tree layout changes; CI gains two integration test binaries.

- [x] **Step 1: Write the ADR.** Status `Accepted`.
- [x] **Step 2: Add it to `CONTEXT.md`'s module-seam note**, replacing the current text at lines 38-46 that says nothing derives the fact from the compiler graph.
- [x] **Step 3: Commit**

```bash
git add docs/adr/0065-*.md CONTEXT.md
git commit -m "docs: record why the source-tree and doc-claim guards come back"
```

**Wave 0 DoD check:** both guards exist and were each observed failing; ADR-0065 committed; spec and plan committed; the `plans/` directory is non-empty. Gate: `cargo test --workspace --lib --bins --tests --locked`, clippy, fmt.

**Wave 0 review:** a fresh reviewer confirms the guards are real by reading `module_declarations` and the link extractor and asking whether each can produce a false PASS. The specific risk: a `mod` declaration inside a `#[cfg(test)]` block or a doc comment being counted, which would make a genuinely orphaned file pass.

---

# Wave 1 — Dead surface, wiring, duplicates

Behaviour-preserving. Nothing here changes what the server does; it changes what exists and what is reachable.

## Task 1.1: Baseline the suppression list, then cut the dead public surface

**Files:**
- Modify: `crates/memory-mcp/src/service.rs` (remove lines 102-124)
- Modify: `crates/memory-mcp/src/service/query.rs` (remove `decayed_confidence`, lines 16-19, and its 4 tests at 28-131)
- Modify: `crates/memory-mcp/src/service/core.rs` (remove `get_surrealdb_config`, lines 367-371, and its test at 927-939)
- Modify: `crates/memory-mcp/src/storage/client.rs` (remove lines 435-469; collapse `server_version` 473-493 and `sql_query_take` 680-692)
- Modify: `crates/memory-mcp/src/service/mock_db.rs` (fix the doc example at lines 4-9)

**Interfaces:**
- Consumes: nothing.
- Produces: a baseline list of every `#[allow(dead_code)]` in the crate, recorded in the commit message, which DoD item 4 checks against for all later waves.

Ruling: cut the block, keep no alias. `memory-mcp` is a library crate, but `eval-harness` is the only external consumer and it imports from `crate::knowledge::` and `crate::models::` paths directly. The `service::` prefix is a naming convention with zero callers.

- [ ] **Step 1: Record the baseline**

Run: `grep -rn "#\[allow(dead_code" crates/*/src | sort`
Expected: exactly these — `storage/client.rs` ×4, `memory/inbox_revision_store.rs:27,35`, `http/registry/migrations.rs:40`, `embedding/model_artifacts/state.rs:114`, `service/apps/dispatch.rs:92`, `models/inbox_revision.rs:142,151`, `knowledge/claims_policy/telemetry.rs:20,96`, `knowledge/entity_extraction/gliner.rs:147,1128`, plus the `tests/common/mod.rs` ones handled in Task 1.3 and the module-level `#![allow(dead_code)]` headers on feature-gated modules. Paste the list into the commit message.

- [ ] **Step 2: Write the test that fails when a re-export has no consumer**

Create `crates/memory-mcp/tests/public_surface_audit.rs`:

```rust
#[test]
fn no_service_module_re_export_has_no_consumer() {
    // Walk crates/memory-mcp/src for any path matching
    //   crate::service::<name>  or  memory_mcp::service::<name>
    // outside crates/memory-mcp/src/service.rs itself.
    // Every hit is a re-export with a live consumer; assert the hit list
    // is a subset of an explicit allowlist in this test.
}
```

This is a ratchet: it prevents the block from regrowing while allowing the handful of names that do have consumers (`build_extract_log_result` is `pub(crate)`; check the current set before writing the allowlist).

- [ ] **Step 3: Run it.** Expected: FAIL, listing the 32 re-exported names.
- [ ] **Step 4: Remove `service.rs` lines 102-124.** Also remove `episode_from_record` from line 106's re-export — `fact_from_record` has one consumer (`tests/apps_ingestion_review.rs:2,68`) and must stay; `episode_from_record` has none and every caller uses `crate::memory::episode::episode_from_record`.
- [ ] **Step 5: Remove `decayed_confidence` from `service/query.rs`.** Its only callers are its own 4 tests. `memory/retrieval.rs:23` `fact_decayed_confidence` is the live function — 13 call sites, passed as a function pointer into `pipeline.rs:521,537,681`, `rescue.rs:224,681,754,837,866`, `experience.rs:165`, `scoring.rs`. Keep that one. It is not a redundant wrapper around `models::Fact::decayed_confidence`: it is the injection point that makes decay substitutable in tests, so removing it would remove the only seam.
- [ ] **Step 6: Remove `get_surrealdb_config` from `core.rs`** and its test at 927-939.
- [ ] **Step 7: Cut the 4 engine accessors — and stop there.** `local_db` (436), `mem_db` (446), `remote_db` (456), `is_local` (467) are `#[allow(dead_code)]` with "Future use" comments and zero callers. **Do not touch `server_version` (477-479) or `sql_query_take` (688-690).** The first draft of this plan said to collapse their three identical arms onto `run_query_take`; that was wrong, and the reason matters. `DbEngine::Local` and `DbEngine::Mem` are both `Arc<Surreal<Db>>` while `DbEngine::Remote` is `Arc<Surreal<Client>>`, so the arms are identical in body but not in type — they exist because the compiler cannot unify `Surreal<Db>` with `Surreal<Client>` behind one binding. Collapsing them would need a `DbEngine::as_connection(&self) -> &Surreal<impl Connection>`, which Rust cannot express without boxing to a trait object, and SurrealDB's `query` requires the concrete `Connection`. The three-arm match is the idiomatic way to say "same operation, three connection types", and `run_query_take`'s `impl Connection` parameter is already what keeps each arm a one-liner. Cutting the 4 accessors is the whole of this step.
- [ ] **Step 8: Fix the `mock_db.rs` doc example.** It passes `vec!["org".into()]` where `MemoryService::new` (`core/builder.rs:477`) takes `active_namespace: String`. The block is `rust,no_run`, which compiles — so this is a compile error the moment the file is built as a doctest. Change to `TEST_ACTIVE_NAMESPACE.to_string()`-equivalent: `"org".to_string()`.
- [ ] **Step 9: Run everything.** `cargo test -p memory_mcp --doc --lib` to confirm the doctest now compiles, then the full suite.
- [ ] **Step 10: Commit**

```bash
git commit -m "refactor(service): cut the re-export block nothing reads, and the four suppressed engine futures"
```

**DoD:** the ratchet test passes; `grep -rn "#\[allow(dead_code" crates/*/src` no longer lists `storage/client.rs`; doctests compile; baseline list in the commit message is unchanged otherwise.

## Task 1.2: Wire the observability stack into CI

`observability/` holds 31 recording rules, 15 alerts, 4 dashboards, and 4 Python checkers. No workflow, Makefile target, or Cargo target reaches them. The checkers read only local files and need no running Prometheus. Three of them need PyYAML, which `pyproject.toml` does **not** declare — it only resolves by accident through the onnxruntime chain.

**Files:**
- Create: `crates/xtask/src/observability.rs`
- Modify: `crates/xtask/src/main.rs` (add `mod observability;` and a `CheckObservability` variant)
- Modify: `pyproject.toml` (add `pyyaml>=6`)
- Modify: `.github/workflows/ci.yml` (add a step to job `quality`)

**Interfaces:**
- Consumes: `observability/{check_rules,check_alerts,check_dashboards,build_dashboards}.py`.
- Produces: `xtask check-observability`, which runs all four in order and exits nonzero if any fails.

- [ ] **Step 1: Write `crates/xtask/src/observability.rs`** with `pub fn run() -> Result<(), String>`. It locates the workspace root from `CARGO_MANIFEST_DIR` (`crates/xtask` → `../..`), then for each of the four scripts runs `python3 <path>`, in this order:

  1. `build_dashboards.py` — regenerates the two dashboard JSONs, so the committed dashboards are provably current.
  2. `check_rules.py` — every recording rule reads a metric family the crate actually exports.
  3. `check_alerts.py` — every alert reads a real series and names where to look.
  4. `check_dashboards.py` — every panel expression reads a series that exists.

  Capture stdout and stderr; on nonzero exit return `Err(format!("{name} failed:\n{stdout}\n{stderr}"))`.

- [ ] **Step 2: Write the failing test** for the xtask subcommand in `crates/xtask/src/observability.rs` as a `#[cfg(test)] mod tests` with a test `run_reports_the_first_failing_script`: point the runner at a temp directory containing a `check_rules.py` that exits 1, assert `run()` returns `Err` naming that script. The runner must take the scripts directory as a parameter so the test can inject it; `main.rs` passes the real one.
- [ ] **Step 3: Run it.** `cargo test -p xtask`. Expected: FAIL (the parameterisation does not exist), PASS after the signature is `run(scripts_dir: &Path) -> Result<(), String>`.
- [ ] **Step 4: Wire the subcommand.** Add to `enum Command` in `main.rs` after line 45: `CheckObservability`, and a match arm that calls `observability::run(...)` and maps `Err` to the existing `eprintln!` + `ExitCode::FAILURE` at lines 60-61.
- [ ] **Step 5: Add `pyyaml>=6` to `pyproject.toml`** dependencies, next to numpy/onnxruntime/tokenizers, with a comment that the observability checkers need it and `gen_anno_onnx_parity.py` needs the rest.
- [ ] **Step 6: Run it locally.** `cargo run -p xtask -- check-observability`. Expected: exit 0 and the four scripts' success lines. If `check_rules.py` fails, it has found a rule reading a metric the crate no longer exports — fix the rule, not the check.
- [ ] **Step 7: Add the CI step** in job `quality` after the clippy step at `ci.yml:51`:

```yaml
- name: Observability rules, alerts and dashboards
  run: |
    python3 -m pip install --quiet pyyaml
    cargo run --locked -p xtask -- check-observability
```

- [ ] **Step 8: Prove the CI step can fail.** Temporarily add a recording rule to `observability/recording_rules.yml` that reads `memory_nonexistent_metric_total`, run the xtask command, confirm it exits 1 with the rule named, then revert.
- [ ] **Step 9: Commit**

```bash
git commit -m "ci(observability): check the rules, alerts and dashboards, so they stop drifting"
```

**DoD:** the xtask subcommand exists, is wired into `quality`, and was observed failing on a deliberately broken rule; `pyproject.toml` declares `pyyaml`; the committed dashboards are byte-identical to what `build_dashboards.py` produces (`git diff --exit-code observability/dashboards/`).

## Task 1.3: Wire the features and the Makefile, cut the dead test helpers

Four facts drive this task. `accelerate` has 0 source sites and 0 CI builds. `mimalloc` has 1 site (`main.rs:1-3`) and 0 CI builds. `metal`'s `ner_metal` bench is compiled by `make bench-check` but never executed by any workflow, and `benchmark-nightly` runs on `ubuntu-24.04` so it cannot execute a Metal bench at all. Nine of sixteen Makefile targets are never invoked.

**Files:**
- Modify: `.github/workflows/ci.yml` (job `native`, near the macOS feature lint at lines 119-121)
- Modify: `.github/workflows/evaluations.yml` (job `benchmark-nightly`, lines 52-77)
- Modify: `Makefile` (delete `serve-release`, `eval-pr`, `eval-release`, `eval-nightly`, `prepare-eval-corpora`, `eval-external-*`, `bench-cpu`, `bench-metal`)
- Modify: `crates/memory-mcp/tests/common/mod.rs` (remove `make_service_with_query_logging` 72-77, `seed_fact_with_links_and_project` 198-253)

**Interfaces:**
- Consumes: nothing from other tasks.
- Produces: a CI matrix row proving `accelerate` and `mimalloc` compile; a Metal bench job that runs `ner_metal` on a Darwin-arm64 runner.

Ruling recap: cut the two dead helpers, keep `seed_fact_with_links` (2 external callers), cut `update_progress_fenced` (Task 1.5).

- [x] **Step 1: Cut the two dead test helpers.** `make_service_with_query_logging` is called only by `make_service` at `mod.rs:64`, which passes `false` — and every real query-log consumer already uses `make_service_with_client_and_query_logging(true)`, verified at `service_integration.rs:943,1019,1077,1142,1205,2249`. `seed_fact_with_links_and_project` has zero callers; its 55 lines exist to cover a `project`/`source_id` combination no test exercises. Remove both and their `#[allow(dead_code)]`.

- [x] **Step 2: Write the test that fails when a test helper is dead.** In `crates/memory-mcp/tests/common/mod.rs`, add a `#[cfg(test)] mod helper_audit` that cannot see its own crate's other test binaries. Instead add `crates/memory-mcp/tests/common_surface.rs`: for every `pub async fn` in `common/mod.rs`, grep `crates/memory-mcp/tests/*.rs` and this file for `common::<name>` and assert at least one hit. Run it, expect FAIL listing the two names removed in Step 1 if you skipped that step, PASS after.

- [x] **Step 3: Add the feature lint row.** In `ci.yml` job `native`, immediately after the macOS feature lint at lines 119-121, add:

```yaml
- name: Apple accelerator and allocator lint
  if: matrix.target == 'aarch64-apple-darwin'
  run: cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http,accelerate,mimalloc --locked -- -D warnings
```

ADR-0034 requires both to stay out of `default`; this row proves they still build without making them default.

- [x] **Step 4: Prove the row can fail.** Temporarily add `use nonexistent_crate as _;` to `src/lib.rs`, run the command, confirm it fails, revert.

- [x] **Step 5: Make `ner_metal` runnable.** `benchmark-nightly` in `evaluations.yml` has no `matrix:` key. Add one, or add a sibling job. The Metal bench needs `memory_mcp/metal` in `--features` and a Darwin-arm64 host (per `Makefile:72-75`). Add to `evaluations.yml`:

```yaml
  benchmark-metal:
    name: Metal NER benchmark
    runs-on: macos-15
    steps:
      - uses: actions/checkout@v7.0.0
      - uses: ./.github/actions/setup
        with: { target: aarch64-apple-darwin }
      - name: Run the Metal NER benchmark
        run: |
          test -d crates/memory-mcp/tests/models/ner || exit 0
          MEMORY_MCP_BENCH_REQUIRE_FIXTURES=1 cargo bench -p eval-harness \
            --features memory_mcp/metal,eval-harness/bench --bench ner_metal --locked
```

The `test -d` guard mirrors the existing CPU bench at `evaluations.yml:71-77`, which skips when fixtures are absent. The `runs-on: macos-15` matches the existing Apple-Silicon row in `ci.yml:103-104`.

- [x] **Step 6: Delete the dead Makefile targets.** Keep `eval-response-size`, `eval-ner-quality`, `bench-check`, `bench-cpu-core` — the four CI actually invokes (`ci.yml:78`, `evaluations.yml:38,41,68,71`). Delete the other twelve entries and their recipes. `eval-pr`, `eval-release`, `eval-nightly` are one-line `cargo run` invocations that `evaluations.yml:34-36` already inlines; `bench-metal`'s recipe moves into the new CI step.

- [x] **Step 7: Commit**

```bash
git commit -m "ci: build the accelerator and allocator features, and run the Metal benchmark"
```

**DoD:** the accelerator/allocator lint row exists and was observed failing; `ner_metal` is executed by a workflow; the Makefile has four targets and all four are CI-invoked; the helper audit test passes; the only `#[allow(dead_code)]` left in `tests/common/mod.rs` are for helpers with real consumers.

## Task 1.4: Collapse the duplicated logic

Nine duplicate pairs. Six are mechanical. Three need a judgement call recorded here.

**Files:**
- Modify: `crates/memory-mcp/src/storage/queries.rs` (delete `normalize_surreal_json` at 535, use the one in `helpers.rs:153`)
- Modify: `crates/memory-mcp/src/knowledge/entity_extraction/gliner.rs` (delete `apply_nms` 1393-1425, call the one in `lfm2_gliner/decode.rs:145` after unifying the input type)
- Modify: `crates/memory-mcp/src/memory/retrieval/lexical.rs` (delete `lexical_record_term_set` 438-458, `matched_query_terms_for_record` 460-490, `lexical_query_overlap` 508-535; use `shared/search_lexical.rs:30,42`)
- Modify: `crates/memory-mcp/src/memory/procedures_service/ranking.rs` (use `shared::search::search_query_terms` at line 72 instead of `split_whitespace()`)
- Modify: `crates/memory-mcp/src/models/claim.rs` (`PolicyFingerprint::compute_v2` is canonical; `compute` at 388 is legacy — verify its callers and delete if none)
- Modify: `crates/memory-mcp/src/memory/agent_memory/recall.rs` (`policy_fingerprint` at 64-68 is a plain sorted join; make it call `PolicyFingerprint::compute_v2`)
- Modify: `crates/memory-mcp/src/service/query.rs` already done in Task 1.1

**Interfaces:**
- Consumes: `shared::search_lexical::{fact_term_set, matched_query_terms_for_fact}` — declared at `shared/search_lexical.rs:3-5` as the home for exactly this loop.
- Produces: one lexical-overlap implementation, one NMS implementation, one value normaliser, one policy-fingerprint hash.

Three judgement calls, decided here:

**`normalize_surreal_json`**: the two copies are in the same module (`queries.rs:535`, `helpers.rs:153`) with identical match arms. Keep the `helpers.rs` one, make it `pub(crate)`, delete the `queries.rs` one and its call site. This one is unambiguous.

**`apply_nms` — do not merge these.** The first draft of this plan said to unify them. Having read both, that would be wrong. `gliner.rs:1393` takes `Vec<ScoredSpan>` and `decode.rs:145` takes `Vec<ScoredEntity>` — and the two structs are field-for-field identical (`start`, `end`, `text`, `label`, `score`; `gliner.rs:111-117` is private, `decode.rs:84-90` is `pub`). The honest reading is that there is one algorithm and two near-identical types, and the duplication is in the *type*, not the function. The correct fix is the opposite direction from what the plan first said: make `ScoredSpan` a type alias of `ScoredEntity` (or delete it and use `ScoredEntity` in `gliner.rs`), then have both call sites use `decode::apply_nms` — which is already `pub(crate)` and already called from `model.rs:1156`. That removes the duplicate type, the duplicate constant `NMS_IOU_THRESHOLD` (gliner.rs:1337) versus `decode.rs`'s own threshold, and the duplicate algorithm, without a conversion shim at the call site. If unifying the types turns out to ripple further than one file, cut `gliner.rs`'s copy and keep the duplication of the *type* — a duplicated struct is far cheaper than a duplicated algorithm, and that is the correct order of concern. Move the NMS tests to the surviving function so coverage does not drop.

**`PolicyFingerprint`**: `models/claim.rs:409` `compute_v2` sorts then hashes. `memory/agent_memory/recall.rs:64` sorts and `join(",")` with no hash and no namespace, and its output is stored verbatim into `ExposureTrace.policy_fingerprint` and into the `RecallKey` string. These are different things wearing one name: one is a fingerprint, the other is a display string. Rename the recall one to `policy_tag_set_key` rather than forcing it through the hash — the recall cache keys on the tag set, not on a digest, and hashing would make the stored trace unreadable. Record the rename in the commit message so a future reader does not "fix" the inconsistency.

**`normalized_task_overlap` — do not merge this either.** `procedures_service/ranking.rs:72` splits on `split_whitespace()` and computes Jaccard. `shared::search::search_query_terms` (search.rs:22) normalises, lowercases and filters. They are not the same operation: the procedure ranker wants raw whitespace tokens, and running them through `search_query_terms` would change which candidates match, silently altering retrieval quality. This is a **finding to record, not a fix to make** — add a comment at `ranking.rs:72` saying why the raw split is deliberate. The first draft of this plan wrongly scheduled this as a change.

- [x] **Step 1: Write a test that fails when a duplicate exists.** Create `crates/memory-mcp/tests/no_duplicate_implementations.rs`: for a table of `(concern, canonical_path, forbidden_names)`, scan `crates/memory-mcp/src/**/*.rs` for `fn <name>` and assert each forbidden name is defined exactly once, in the canonical file. The table: `normalize_surreal_json`→`storage/helpers.rs`; `ScoredSpan`→must not exist anywhere; `apply_nms`→`knowledge/entity_extraction/lfm2_gliner/decode.rs`; `lexical_record_term_set`/`matched_query_terms_for_record`/`lexical_query_overlap`→must not exist anywhere; `policy_fingerprint`→must not exist in `recall.rs`.

  Run: `cargo test -p memory_mcp --test no_duplicate_implementations`
  Expected: FAIL listing each current duplicate.

- [x] **Step 2: Collapse `normalize_surreal_json`.** Make `helpers.rs:153` `pub(crate)`, delete `queries.rs:535` and its call site, run the test.
- [x] **Step 3: Unify the NMS type, then the function.** Delete `ScoredSpan` (gliner.rs:111-117), alias or re-use `ScoredEntity`, point `gliner.rs:1509` at `decode::apply_nms`, delete `gliner.rs:1393-1425` and its `NMS_IOU_THRESHOLD` at 1337. Move `gliner.rs`'s three NMS tests (2146, 2158, 2168) into `decode.rs`'s test module alongside its own (306) so the union is covered. If the type unification ripples beyond `gliner.rs`, stop and take the fallback recorded above — cut the function, keep the type.
- [x] **Step 4: Collapse the lexical loops.** `lexical.rs` operates on `Value` rows, `shared/search_lexical.rs` on `Fact` — that is why the copies exist. Fix it by decoding the `Value` row to a `Fact` first (there is already `knowledge/fact_parsing.rs:12` `fact_from_record`), then calling the shared functions. Delete the three local functions and their tests. Check first that `lexical_record_term_set` and `matched_query_terms_for_record` return the same set the `Fact` versions do — the record version returns `HashSet<String>` from `content` plus `index_keys`, which matches, but `lexical_query_overlap` (508-535) may compute a ratio the shared function does not expose. If that is the only gap, add `overlap_ratio` to `shared/search_lexical.rs` rather than keeping a local copy. This is the largest of the six; do it on its own commit.
- [x] **Step 5: Record the two deliberate non-merges.** Add the comment at `procedures_service/ranking.rs:72` described above. Add a comment at `memory/retrieval/temporal.rs:589` recording why temporal candidate ordering is separate from `ranking.rs:168` fusion.
- [x] **Step 6: Fix the two fingerprints.** Delete `PolicyFingerprint::compute` (claim.rs:388) if `grep -rn "PolicyFingerprint::compute(" crates/` shows no caller. Rename `recall.rs:64` to `policy_tag_set_key` and update its 3 call sites (`:379`, `:41`, `:56`).
- [x] **Step 7: Three ranking paths remain deliberately.** `memory/retrieval/ranking.rs:168` (RRF fusion) ranks facts for the assembled context; `memory/retrieval/temporal.rs:589` (lexical+recency) orders candidates inside a temporal window; `procedures_service/ranking.rs:31` ranks procedure candidates by posterior mean. Three different objectives. Do not merge these. Step 5 records why, so a later reader does not propose a fourth consolidation.
- [x] **Step 8: Run the duplicate test.** Expected: PASS. Then the full suite.
- [x] **Step 9: Commit** — one commit per step group, so a reviewer can reject step 3 without rejecting step 2.

```bash
git commit -m "refactor(search): one lexical-overlap loop, in the module that already owns it"
```

**DoD:** the duplicate test passes; the seven duplicate pairs are gone; coverage for NMS and lexical scoring is not lower than before (compare test counts per file); the two deliberate ranking paths are documented in the commit message.

## Task 1.5: Cut the dead trait method

**Files:**
- Modify: `crates/memory-mcp/src/http/tasks/state.rs` (remove `update_progress_fenced`, lines 111-120)
- Modify: `crates/memory-mcp/src/http/tasks/worker.rs` (remove its impl at 316)

**Interfaces:**
- Consumes: nothing.
- Produces: a `TaskStore` trait of 10 methods, each with at least one caller.

`update_progress_fenced` has zero call sites — not in production, not in a test, not in the test driver at `http/tasks.rs`. Every other one of the 11 methods has at least one. Removing it is not narrowing the trait by a choice; it removes a method whose behaviour was never specified by a caller.

- [x] **Step 1: Write the test that fails when a trait method is uncalled.** Create `crates/memory-mcp/tests/trait_methods_are_called.rs`: for each method named in a table `(trait_path, method_name)`, grep `crates/memory-mcp/src` and `crates/memory-mcp/tests` for `.method_name(` and assert at least one hit that is not the trait declaration and not the impl body. Table entries: the 11 `TaskStore` methods, the 19 `LocalAdminStore` methods, the 8 registry owner traits' 51 methods.

  Run: expect FAIL listing `update_progress_fenced`. Every other entry must PASS — if another fails, that is a new finding: record it and cut it too, or wire it, per the ruling.
- [x] **Step 2: Remove the declaration and the impl.**
- [x] **Step 3: Run the test.** Expected: PASS.
- [x] **Step 4: Commit**

```bash
git commit -m "refactor(tasks): drop update_progress_fenced, which no caller ever asked for"
```

**DoD:** the trait-method test passes and covers all 81 methods; the two files compile; no behaviour change.

## Task 1.6: Wire the retrieval test fakes onto MockDbClient

This is the one item where "wire rather than delete" applies literally, and the first draft of this plan promised it in the audit round without scheduling it. `memory/retrieval.rs` is 1690 lines of which 1327 (78%) are tests, and eight of those tests each hand-write a full six-method `DbClient` impl — `FallbackTierDbClient` (462), `EpisodeContentFallbackDbClient` (567), `CommunityLookupDbClient` (672), `FusionDbClient` (859), `CommunityRankingDbClient` (998), `CommunityOriginWeightDbClient` (1137), `SemanticDbClient` (1305), `EmptyCommunityFactDbClient` (1440). That is roughly 500 lines of duplicated fake. Meanwhile `MockDbClient` has four builders no test calls: `expect_select_table` (103), `expect_select_table_with` (111), `expect_select_table_panic` (119), `expect_migration_result` (126).

**But `MockDbClient` cannot absorb them as it stands, and the plan does not pretend otherwise.** Its `query` method (`mock_db.rs:235-242`) has no builder, no response map, and no fallback slot — it returns `Ok(Value::Null)` unconditionally. Every one of the six fakes that carries retrieval behaviour dispatches on `query`. So the consolidation requires giving `MockDbClient` a `query` seam first, and only some of the fakes then fit.

| Fake | Fits? | Why |
|---|---|---|
| `EmptyCommunityFactDbClient` (1440) | **yes, zero new capability** | every method already returns a default `MockDbClient` returns. `retrieval.rs:823` already proves the equivalence. |
| `EpisodeContentFallbackDbClient` (567) | yes, with `expect_query_with` | one stateless branch on a SQL substring. |
| `CommunityRankingDbClient` (998) | yes, same | two stateless substring branches. |
| `SemanticDbClient` (1305) | yes | needs the query fallback plus the existing `expect_select_table_panic("fact")` for its negative assertion at 1322. |
| `FallbackTierDbClient` (462) | yes, with care | dispatches on `vars["query"]` per term. A closure over `(&str sql, Option<Value> vars)` reproduces it. |
| `FusionDbClient` (859) | yes, with care | same, plus a `panic!` on an unexpected term at 924 — the closure can do that. |
| `CommunityOriginWeightDbClient` (1137) | yes | three branches, one keyed on `vars["node_id"]`. |
| `CommunityLookupDbClient` (672) | **no** | needs call counters (`community_lookup_calls`, `entity_link_fact_calls`, asserted at 815-816) and a `query`-side panic primitive. A stateless keyed-response model has nowhere to record a count. |

Two tests (1527, 1605) use a real in-memory SurrealDB and must stay — they exist to exercise real FTS and bi-temporal visibility, and moving them onto a mock would delete the coverage they were written for.

**Files:**
- Modify: `crates/memory-mcp/src/service/mock_db.rs` (add a query seam; wire the 4 unused builders)
- Modify: `crates/memory-mcp/src/memory/retrieval.rs` (delete 7 fakes; keep `CommunityLookupDbClient` and the 2 real-DB tests)
- Test: `crates/memory-mcp/src/service/mock_db.rs` (its own `mod tests`), `crates/memory-mcp/src/memory/retrieval.rs`

**Interfaces:**
- Consumes: `DbClient::query(&self, sql: &str, vars: Option<Value>, namespace: &str)` (`storage/client.rs:63`).
- Produces:

```rust
impl MockDbClient {
    /// Script `query` by SQL substring. The responder sees the SQL and the
    /// bound variables, so a test can dispatch on either — which is what the
    /// retrieval fakes need, because they key on `vars["query"]` and
    /// `vars["node_id"]` rather than on the table.
    pub fn expect_query_with(
        mut self,
        matches: impl Fn(&str) -> bool + Send + Sync + 'static,
        respond: impl Fn(&str, Option<&Value>) -> Result<Value, MemoryError> + Send + Sync + 'static,
    ) -> Self;

    /// Panic if any query reaches `query`, naming the SQL. The `query`
    /// counterpart to `expect_select_table_panic`, for a test that asserts a
    /// code path takes no query at all.
    pub fn expect_no_query(self) -> Self;
}
```

`expect_query_with` takes a predicate and a responder rather than a single closure because several fakes branch on three different SQL shapes; one closure with an internal match would also work, but the two-parameter form states the intent — "when the SQL looks like this, answer like that" — and leaves room for a non-matching SQL to fall through to the next rule rather than panicking inside one handler.

- [x] **Step 1: Write the failing test** in `mock_db.rs`'s own test module: `query_dispatches_on_sql_and_vars` — build a client with two `expect_query_with` rules, one matching `search::score` and one matching `FROM community`, call `query` with each, assert the two different responses come back and that an unmatched SQL returns the default `Value::Null`.

  Run: `cargo test -p memory_mcp --lib mock_db`. Expected: FAIL — the builder does not exist.
- [x] **Step 2: Add the query seam** to `MockDbClient`: a `Mutex<Vec<QueryRule>>` field, where `QueryRule` holds the predicate and responder as boxed `Fn` trait objects to match the file's existing `SelectOneFn` style at lines 20-24. The `DbClient::query` impl walks the rules in order and returns the first match. Add `expect_no_query` as a rule that matches everything and panics. Note that `mock_db.rs` is `#[cfg(test)]`-only (`service.rs:78-79`), so a `panic!` here cannot reach production — the same rule `expect_select_table_panic` already relies on.
- [x] **Step 3: Run the test.** Expected: PASS.
- [x] **Step 4: Migrate the six fakes that fit.** For each: delete the struct and its `impl DbClient`, and express the same behaviour as a `MockDbClient` builder chain at the construction site. The tests' assertions do not change, so if one starts failing, the fake was doing something the mock cannot — stop and record which, rather than weakening the assertion. This is the check that the consolidation is behaviour-preserving.
- [x] **Step 5: Keep `CommunityLookupDbClient`** and add a comment at its definition saying why it stays: it is the only test that asserts *how many times* a query shape is issued, which a stateless responder cannot express. If a later change makes call-counting expressible — a responder that receives a shared counter — migrate it then.
- [x] **Step 6: Wire the four previously-unused builders.** `expect_select_table` and `expect_select_table_with` are needed by the `SemanticDbClient` migration (its `select_table` assertion at 692). `expect_select_table_panic` is needed by the same test's negative case. `expect_migration_result` has no consumer in this task — it is the one builder with no plausible home here, so cut it and note that in the commit message rather than leaving a fourth unused builder behind.
- [x] **Step 7: Run `cargo test -p memory_mcp --lib retrieval`.** Expected: PASS, with the same number of retrieval tests as before. Count them before and after: this task must not reduce coverage, and a lower count means a test was deleted rather than migrated.
- [x] **Step 8: Commit**

```bash
git commit -m "test(retrieval): seven hand-written database fakes become one scriptable mock"
```

**DoD:** `retrieval.rs` is roughly 500 lines shorter; the retrieval test count is unchanged; `expect_query_with` and `expect_no_query` are covered by their own tests; `expect_migration_result` is gone and the other three builders have callers; `CommunityLookupDbClient` remains with a comment explaining why.

**Wave 1 DoD check:** every item on the dead-surface list is gone or wired; the observability check runs in CI and was observed failing; `accelerate`/`mimalloc` build in CI; `ner_metal` runs; the Makefile has four CI-invoked targets; no duplicate pair remains; every `plans/` citation resolves; both spec status lines match the code; the retrieval fakes are consolidated. Gate: full suite, clippy, fmt, `cargo test -p xtask`.

**Wave 1 review:** a fresh reviewer checks that no behaviour changed. Three specific risks: (1) `normalize_surreal_json` and the lexical loops are not byte-identical in edge cases — diff the match arms, do not assume; (2) Task 1.1's ratchet test must actually fail if a re-export is added back, which the reviewer verifies by adding one and re-running; (3) Task 1.6's migrated fakes must not have had an assertion weakened to make a conversion fit — the reviewer reads each diff hunk in `retrieval.rs` and asks whether the test still proves what it proved before.

---

# Wave 2 — One embedding path

## Task 2.1: Make `embedding::api` the only generation interface

Four callers generate vectors. Two route through `EmbeddingService::generate_embedding`; `fact_orchestration.rs:95` inlines its own payload build and skips the write policy; `embedding_recovery.rs:253` calls `provider.embed()` directly, bypassing truncation (`embedding/service.rs:109-129`), the disabled check (`:138`) and logging (`:172-208`).

**Files:**
- Modify: `crates/memory-mcp/src/embedding/api.rs` (add `generate_and_update`)
- Modify: `crates/memory-mcp/src/service/fact_orchestration.rs` (delete the inlined build, call the new function)
- Modify: `crates/memory-mcp/src/service/embedding_recovery.rs` (replace the direct provider call)
- Modify: `crates/memory-mcp/src/service/reembed.rs` (call the new function)
- Modify: `crates/memory-mcp/src/embedding/service.rs` (background retry calls the new function)
- Test: `crates/memory-mcp/tests/embedding_vector_policies.rs`

**Interfaces:**
- Consumes: `embedding::api::update_canonical_vector(store, fact_id, vector, policy)` at `api.rs:139`; `VectorWritePolicy::{FillMissing, ReplaceStale}`; `EmbeddingService::generate_embedding` at `service.rs:109`.
- Produces:

```rust
/// Generate the vector for `fact_id` and write it under `policy`.
///
/// Every vector the system writes goes through this one function, so the
/// truncation limit, the disabled-provider check and the progress logging
/// inside `EmbeddingService` cannot be skipped by a caller that already holds
/// a provider.
pub async fn generate_and_update(
    embedding: &(impl EmbeddingGeneration + ?Sized),
    store: &(impl VectorWritePort + ?Sized),
    input: &EmbeddingInput,
    policy: VectorWritePolicy,
) -> Result<EmbeddingOutcome, MemoryError>;
```

`EmbeddingGeneration` is a port declared in `embedding/api.rs` with one method `generate_embedding(&self, input: &EmbeddingInput) -> Result<Vec<f32>, MemoryError>` — satisfied by `EmbeddingService` (its existing method, unchanged) and by an in-memory fake in tests. `EmbeddingOutcome` is a new enum: `Written { dimension: usize }` or `Skipped { reason: SkipReason }`, where `SkipReason` is a bounded enum (`ProviderDisabled`, `AlreadyPresent`, `NotEmbeddable`) so it can become a metric label without an unbounded string.

- [x] **Step 1: Write the failing test** in `crates/memory-mcp/tests/embedding_vector_policies.rs`:

```rust
#[tokio::test]
async fn a_disabled_provider_skips_the_write_and_reports_why() {
    let store = InMemoryVectorStore::default();
    let outcome = embedding::api::generate_and_update(
        &DisabledEmbeddingProvider::new(), &store,
        &test_input(), VectorWritePolicy::FillMissing,
    ).await.expect("a disabled provider is not an error");
    assert!(matches!(outcome, EmbeddingOutcome::Skipped { reason: SkipReason::ProviderDisabled }));
    assert_eq!(store.writes(), 0);
}

#[tokio::test]
async fn long_content_is_truncated_before_generation() {
    // input longer than the truncation limit; assert the generator received
    // exactly the truncated string, using a recording EmbeddingGeneration.
}
```

`InMemoryVectorStore` is a second adapter for `VectorWritePort`, which is what makes this seam real. Run: `cargo test -p memory_mcp --test embedding_vector_policies`. Expected: FAIL — `generate_and_update` does not exist.

- [x] **Step 2: Declare the two ports and the outcome enum** in `embedding/api.rs`. Follow the house style in `operations/api.rs`: `#[async_trait::async_trait]`, `Send + Sync`, doc comment stating why.
- [x] **Step 3: Implement `generate_and_update`**: call `embedding.generate_embedding(input)`, then `store.update_canonical_vector(fact_id, vector, policy)`. Translate the existing disabled-check and truncation into the `SkipReason` and the `EmbeddingInput` respectively, so both live inside this function.
- [x] **Step 4: Run the test.** Expected: PASS.
- [x] **Step 5: Migrate the four callers**, one commit each. `embedding_recovery.rs` is the important one: delete the direct `provider.embed()` at line 253 and the inlined `build_embedding_payload` at 252, and call `generate_and_update` with `VectorWritePolicy::FillMissing` — preserving ADR-0042's rule that a compatible recovery uses `backfill_pending` and a signature-mismatch recovery keeps semantic retrieval degraded.
- [x] **Step 6: Write the test that fails when a caller bypasses the path.** Add to `crates/memory-mcp/tests/embedding_vector_policies.rs`: scan `crates/memory-mcp/src` for `.generate_embedding(` and `.embed(` and assert every hit is inside `crates/memory-mcp/src/embedding/`. Run, expect it to fail on the two pre-migration callers, pass after Step 5.
- [x] **Step 7: Commit**

```bash
git commit -m "refactor(embedding): one generation interface, so truncation and logging stop being optional"
```

**DoD:** the four callers go through `embedding::api`; the bypass scan passes; `VectorWritePolicy::FillMissing` and `ReplaceStale` are both covered by tests; no caller outside `src/embedding/` names `generate_embedding` or `provider.embed`; ADR-0042's two recovery behaviours are still tested.

**Wave 2 DoD:** one generation path, four callers, two write policies, both tested. Gate: full suite, clippy, fmt.

---

# Wave 3 — Policy in the owning context

ADR-0066 is written in Task 3.1. Four policies leave the HTTP transport adapter. All four become pure functions in their owning context's `api.rs`, called by both store adapters and by the router.

The ruling: do not narrow `LocalAdminStore`, `TaskStore`, or `AppSessionStore` beyond what this move requires. Two are untouched entirely.

## Task 3.1: Move the quota policy to `operations/api.rs`

**Files:**
- Create: `crates/memory-mcp/src/operations/quota.rs`
- Modify: `crates/memory-mcp/src/operations.rs` (add `pub mod quota;`)
- Modify: `crates/memory-mcp/src/http/registry/plan.rs` (delete `Plan`, `UsageCounter`, `QuotaDecision`, `ReconcilerReport`, `enforce_ingest`, `reconcile_usage`; keep the `From<&models::Plan>` conversion as `operations::quota::Plan::from_contract`)
- Modify: `crates/memory-mcp/src/http/registry/storage.rs:1894` and `src/http/registry/surreal_store.rs:2512` (call the moved function)
- Test: `crates/memory-mcp/tests/knowledge_read_scopes.rs` is the wrong home; add `crates/memory-mcp/tests/quota_policy.rs`

**Interfaces:**
- Consumes: `models::registry::{PlanLimits, DEFAULT_*}` for the `From` conversion.
- Produces:

```rust
// operations/quota.rs
pub fn enforce_ingest(
    plan: &QuotaPlan,
    counter: &mut UsageCounter,
    source_bytes: u64,
    now: chrono::DateTime<chrono::Utc>,
) -> QuotaDecision;

pub fn usage_drift_report(
    plan: &QuotaPlan,
    tenant_id: &str,
    source_count: u32,
    counter_count: u32,
) -> UsageDriftReport;
```

`QuotaPlan` is the old `Plan` renamed — the name `Plan` collides with `models::registry::Plan` and forces a `From` import at every call site. `reconcile_usage` is renamed `usage_drift_report` because the `UsageStore` trait method already owns the bare name (Review Focus item 5).

- [ ] **Step 1: Write ADR-0066** with Status `Accepted`. Context: the four policies and where each lives today. Decision: policy lives in the owning context; the store adapter supplies state and performs the write. Consequences, stated honestly: the quota predicate now exists in two places — a SQL `WHERE` clause at `surreal_store.rs:2482` for atomicity, and the Rust function for the typed denial reason — and the two must agree; ADR-0066 requires a test that pins them together (Task 3.3). Alternative considered and rejected: keep the policy in the store trait, which would make each new adapter re-implement it.

- [ ] **Step 2: Write the failing test** in `crates/memory-mcp/tests/quota_policy.rs`, moving the 6 existing cases from `plan.rs:320-430` verbatim (same names: `ingest_allows_under_limit`, `quota_exceeded_rejects_ingest_with_retry_guidance`, `zero_per_minute_disables_ingest`, `window_rolls_after_60s`, plus the two reconciler cases), retargeted at `operations::quota`. Add one new case per DoD:

```rust
#[test]
fn a_denial_never_mutates_the_counter() {
    // Deny leaves ingest_current_minute, ingested_bytes and episode_count
    // exactly as they were. Currently true by construction — the increments
    // are the last three statements before the Allow return — and it must
    // stay true, because InMemoryStore relies on it for atomicity.
}

#[test]
fn every_denial_reason_is_a_bounded_token() {
    // Assert reason matches ^[a-z_]+$ so it can be a metric label.
}
```

Run: `cargo test -p memory_mcp --test quota_policy`. Expected: FAIL — module does not exist.

- [ ] **Step 3: Create `operations/quota.rs`** with the moved types and functions, plus the `From<&models::registry::Plan>` conversion. `QuotaDecision::is_deny` comes with it.
- [ ] **Step 4: Repoint the two adapters.** `storage.rs:1894` and `surreal_store.rs:2512` call `operations::quota::enforce_ingest`. In `InMemoryStore` the counter increment **is** the store write, so the call stays inside the lock exactly as it is; in `SurrealRegistryStore` the call stays on a discarded local copy, producing the denial reason. Do not "clean up" the second one into a shared code path — the two adapters genuinely differ here, and that difference is ADR-0066's stated consequence.
- [ ] **Step 5: Delete the moved code from `plan.rs`.** Keep `scheduler_job` and `reconcile_all` there; they are HTTP scheduler wiring, not policy. Retarget their imports.
- [ ] **Step 6: Run the test.** Expected: PASS, all 8 cases.
- [ ] **Step 7: Commit**

```bash
git commit -m "refactor(operations): the quota policy lives with the context that owns usage"
```

## Task 3.2: Move the Tenant status transition table to `provisioning/api.rs`

**Files:**
- Modify: `crates/memory-mcp/src/provisioning/api.rs` (add `can_transition` and the `TenantLifecyclePort` trait)
- Modify: `crates/memory-mcp/src/http/registry/provisioning.rs` (delete `can_transition` 78-105, `transition` 31-46, `transition_fenced` 52-74; keep `enqueue_provisioning`, `reconciliation_scheduler_job`, `reconcile`)
- Test: `crates/memory-mcp/tests/registry_store_behaviour.rs`

**Interfaces:**
- Consumes: `models::registry::TenantStatus` (8 variants, `Copy`).
- Produces:

```rust
// provisioning/api.rs
pub fn can_transition(from: TenantStatus, to: TenantStatus) -> bool;

#[async_trait::async_trait]
pub trait TenantLifecyclePort: Send + Sync {
    async fn update_tenant_state(
        &self, tenant_id: &str, expected_version: u64,
        from: TenantStatus, to: TenantStatus,
    ) -> Result<u64, MemoryError>;
    async fn update_tenant_state_fenced(
        &self, tenant_id: &str, expected_version: u64,
        from: TenantStatus, to: TenantStatus, lease: &ProvisioningLease,
    ) -> Result<u64, MemoryError>;
}

pub async fn transition_tenant(
    port: &(impl TenantLifecyclePort + ?Sized),
    tenant_id: &str, expected_version: u64,
    from: TenantStatus, to: TenantStatus,
) -> Result<u64, MemoryError>;

pub async fn transition_tenant_fenced(
    port: &(impl TenantLifecyclePort + ?Sized),
    tenant_id: &str, expected_version: u64,
    from: TenantStatus, to: TenantStatus, lease: &ProvisioningLease,
) -> Result<u64, MemoryError>;
```

`TenantLifecyclePort` is a **real** seam: `TenantStore` satisfies it today, and `InMemoryStore` in tests is the second adapter. The two methods are exactly what the two use cases need, which is the point — `TenantStore` has 11 methods and these use cases need 2.

- [ ] **Step 1: Move the 8 table tests** from `provisioning.rs:307-365` to `tests/registry_store_behaviour.rs`, same names, retargeted. Add:

```rust
#[tokio::test]
async fn an_illegal_transition_is_refused_before_the_store_is_touched() { /* ... */ }
```

Run: expect FAIL, module does not exist.

- [ ] **Step 2: Add the port and the two use cases** to `provisioning/api.rs`, following the file's existing style: `pub async fn use_case(port: &(impl XPort + ?Sized), ...) -> Result<u64, MemoryError>`, doc comment stating the invariant ("the transition is validated before the compare-and-set, so an illegal pair never reaches storage").
- [ ] **Step 3: Implement `TenantLifecyclePort for TenantStore`** — a blanket impl, so both store adapters get it without a second declaration.
- [ ] **Step 4: Repoint `migration.rs`.** Its 5 `transition_fenced` calls (288, 300, 316, 399, 448) become `transition_tenant_fenced`. Its import at line 23 changes.
- [ ] **Step 5: Delete the old functions** from `http/registry/provisioning.rs`. Note that `transition` (the unfenced one) has **zero production callers** — only 3 tests at `provisioning.rs:373,391,405`. Task 3.4 wires it to the operator handlers, which is what makes it earn its place; until then it is ported, not deleted.
- [ ] **Step 6: Run the test.** Expected: PASS.
- [ ] **Step 7: Commit**

```bash
git commit -m "refactor(provisioning): the Tenant transition table lives with the context that owns Tenant"
```

## Task 3.3: Pin the durable quota predicate to the policy function

The durable store enforces the quota in SQL at `surreal_store.rs:2482`; the moved function runs afterwards only to name the denial. If the two drift, a tenant gets refused for a reason that is not the one that stopped it, and `retry_after_secs` lies.

**Files:**
- Test: `crates/memory-mcp/tests/http_registry_storage.rs`

**Interfaces:**
- Consumes: `operations::quota::{QuotaPlan, UsageCounter, QuotaDecision}`.
- Produces: no new production code. This task is a test.

- [ ] **Step 1: Write the failing test** (Review Focus item 3):

```rust
#[tokio::test]
async fn quota_predicate_matches_the_context_policy() {
    // For each denial reason QuotaDecision can carry, construct a plan and
    // counter that the pure function denies for that reason, push the same
    // pair through the durable store's reserve_ingest_usage, and assert the
    // returned reason is the same string.
    // Cases: ingest_disabled, ingested_bytes_exceeded, episode_count_exceeded,
    //        ingest_rate_exceeded
}
```

Build the durable store with the existing helper at `http_registry_storage.rs:206` (`HttpProductionComposition::connect` with `mem://` targets). Run: expect FAIL if any reason string diverges; PASS if they already agree — and if it passes on the first run, say so in the commit, because a test that has never failed has not been shown to test anything. Then mutate one reason string in `operations/quota.rs`, confirm it fails, revert.

- [ ] **Step 2: Also assert the byte drift threshold.** `reconcile_all` at `plan.rs:291` compares byte drift against `plan.reconciler_drift_threshold` inline, while `usage_drift_report` compares episode-count drift. Add a case pinning both to the same constant.
- [ ] **Step 3: Commit**

```bash
git commit -m "test(quota): the SQL predicate and the context policy cannot drift"
```

## Task 3.4: Make the operator handlers obey the transition table

`control/operator.rs` calls `store.update_tenant_state` directly, so `can_transition` is never consulted by any operator path. Two of the three handlers have a real hole, and the two are different in kind — read the handler before changing it:

- **`suspend_tenant` (138-175)** rejects only `Deleting` and `Purged`, then writes `tenant.status -> Suspended`. That accepts `Reserved -> Suspended`, `NamespaceCreating -> Suspended` and `Migrating -> Suspended`, none of which are in the table. The table has no `X -> Suspended` edge except `Ready -> Suspended`.
- **`resume_tenant` (177-197)** never reads `tenant.status` at all. It loads the tenant, then writes `Suspended -> Ready` regardless. Because the store's compare-and-set is on `expected_version` and not on `from`, a `Migrating` tenant resumed here becomes `Ready` while the provisioning loop still holds a lease on it.
- **`retry_tenant` (110-135)** is already correct: it gates on `status == Failed` and uses `retry_stage`, and `Failed -> NamespaceCreating` and `Failed -> Migrating` are both legal. Route it through the same function for consistency, and the test asserts it still works.

**Files:**
- Modify: `crates/memory-mcp/src/control/operator.rs` (3 call sites, at 124, 160, 183)
- Test: `crates/memory-mcp/tests/http_control_plane.rs`

**Interfaces:**
- Consumes: `provisioning::api::{can_transition, transition_tenant, TenantLifecyclePort}`.
- Produces: no new production interface. The operator handlers gain the same validation the scheduler path has.

- [ ] **Step 1: Write the failing test**

```rust
#[tokio::test]
async fn operator_transitions_obey_the_transition_table() {
    // 1. A Ready tenant suspends (Ready -> Suspended, legal) and resumes
    //    (Suspended -> Ready, legal). Both 204.
    // 2. A Migrating tenant cannot be suspended: assert the response is 409
    //    and the tenant's status is still Migrating.
    // 3. A Migrating tenant cannot be resumed into Ready: assert 409 and
    //    status still Migrating.  <- this is the worst one; the tenant would
    //    otherwise report Ready while a provisioning lease is held.
    // 4. A Failed tenant retries into retry_stage and is accepted, so the
    //    fix is not over-broad.
}
```

Run: expect FAIL on cases 2 and 3 — today the direct `update_tenant_state` succeeds, so the status changes and the assertion fails. Cases 1 and 4 must already pass; if they do not, the handler is broken in a way this task did not predict, and stop and report it.

- [ ] **Step 2: Repoint the three handlers** through `transition_tenant`. `transition_tenant` returns `MemoryError::Validation` for an illegal pair; map that to `ApiError::Conflict` (409), which is what both handlers already return for a status they refuse, so the HTTP contract does not change for any existing case. Note that `resume_tenant`'s current behaviour — ignoring the actual status — means case 3 changes an API response from 204 to 409. That is the fix, not a regression; record it in the commit message.
- [ ] **Step 3: Run the test.** Expected: PASS on all four cases.
- [ ] **Step 4: Commit**

```bash
git commit -m "fix(control): an operator transition cannot skip the Tenant transition table"
```

**DoD:** all four cases pass; `suspend_tenant` and `resume_tenant` consult the table; `retry_tenant` is routed through the same function and still works; the response-code change for case 3 is recorded in the commit message.

**Wave 3 DoD:** all four policies live in their owning `api.rs`; both store adapters call the context function; no policy test needs an embedded engine except Task 3.3's, which exists precisely to pin the durable SQL predicate; the operator hole is closed. Gate: full suite, clippy, fmt.

**Wave 3 review:** the reviewer checks that moving the policy did not accidentally make the durable store's atomicity depend on the Rust check. The specific risk is `InMemoryStore::reserve_ingest_usage` at `storage.rs:1885-1900`, where the counter increment inside the lock IS the write — a reviewer must confirm the moved function still mutates the live entry, not a copy.

---

# Wave 4 — Domain SQL and the table allowlist

## Task 4.1: Give every table one allowlist owner

`validate_table_name` (`client.rs:938`) allows 10 tables. The live schema has 23. `select_table` on `claim`, `claim_job`, `claim_key_alias`, `claim_policy`, `claim_relation`, `embedding_job`, `embedding_state`, `entity_extraction_projection`, `event_projection_job`, `memory_capture_audit`, `memory_event`, `procedure_candidate`, or `triple` returns `ConfigInvalid` today.

Ruling: one list per owning context, next to the SQL that touches the table. Not a derived central list, and not a central list kept in sync.

**Files:**
- Modify: `crates/memory-mcp/src/storage/client.rs` (remove `validate_table_name` and `ALLOWED_TABLES`; change `select_table` to take the table from a type that can only be built by an owner)
- Create: `crates/memory-mcp/src/knowledge/table_scope.rs`, `crates/memory-mcp/src/memory/table_scope.rs`
- Modify: `crates/memory-mcp/src/knowledge/{knowledge_store,graph_store,claims}.rs` and `src/memory/episode_context_store.rs` (4 `select_table` call sites)
- Test: `crates/memory-mcp/tests/typed_record_accessors.rs`

**Interfaces:**
- Consumes: `storage::EXPECTED_SCHEMA_TABLES` — expose it from `storage/migrations.rs:342` as `pub(crate)` so the test can compare against it.
- Produces:

```rust
// storage/client.rs
/// A table name that the owning bounded context released.
///
/// The constructor is crate-private, so no module outside this crate can
/// invent a table, and `pub(crate)` is what forces every call site to go
/// through its context's own `TableOwner` impl rather than naming a string.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OwnedTable(pub(crate) &'static str);

impl OwnedTable {
    pub(crate) const fn as_str(&self) -> &'static str {
        self.0
    }
}

/// A bounded context's claim on its tables. A test asserts the union of every
/// impl equals the schema's table list exactly, so a new table cannot be added
/// to one place only.
pub trait TableOwner {
    const OWNED_TABLES: &'static [&'static str];
}

/// Release one of this context's tables. Declared as an inherent associated
/// function on the impl, not a free `OwnedTable::new`, so the only way to build
/// the value is through a type that has declared the table in its
/// `OWNED_TABLES`.
pub trait ReleaseOwnedTable: TableOwner {
    fn table(name: &'static str) -> OwnedTable;
}
```

**The first draft of this plan had `OwnedTable::new` as a `pub const fn`, and that was a design error.** A `const fn` cannot look anything up, so it cannot check that the name belongs to the caller — it would be a newtype around a `&'static str` that anyone can construct, which is exactly the `ALLOWED_TABLES` problem wearing a type-safe costume. It provides no invariant; it only makes the compiler print a nicer type name.

The shape above fixes that in two ways. `OwnedTable`'s field is `pub(crate)`, so only this crate can build one by field access. And construction goes through `ReleaseOwnedTable::table`, an associated function on the `TableOwner` impl, which each context implements once as a `debug_assert!(Self::OWNED_TABLES.contains(&name))` plus the wrap. In a debug build the assertion catches a context reaching for a table it never claimed; in release it is elided, and the test in Step 1 is the real gate. That is the honest division: the type prevents *outside* callers, the `debug_assert` catches *inside* mistakes during development, and the test is the invariant that holds in CI.

- [ ] **Step 1: Write the failing test** (Review Focus item 1) in `crates/memory-mcp/tests/typed_record_accessors.rs`:

```rust
#[test]
fn every_expected_schema_table_has_exactly_one_owner() {
    // Collect every OwnedTable name reachable from the OWNED_TABLES impls.
    // Assert the set == EXPECTED_SCHEMA_TABLES, and that no name appears twice.
}

#[tokio::test]
async fn every_expected_schema_table_is_selectable() {
    // For each name in EXPECTED_SCHEMA_TABLES, build an OwnedTable through its
    // owner's impl and call select_table. Assert no ConfigInvalid.
}
```

Run: expect FAIL — `OwnedTable` does not exist, and the second test would fail today on 13 tables.

- [ ] **Step 2: Add `OwnedTable`, `TableOwner` and `ReleaseOwnedTable`** to `storage/client.rs`, exactly as specified above. Each context's `ReleaseOwnedTable` impl is three lines:

```rust
impl ReleaseOwnedTable for KnowledgeTables {
    fn table(name: &'static str) -> OwnedTable {
        debug_assert!(
            Self::OWNED_TABLES.contains(&name),
            "knowledge reached for a table it does not own: {name}"
        );
        OwnedTable(name)
    }
}
```

- [ ] **Step 3: Implement `TableOwner`** for `knowledge` and `memory`, listing their tables. `storage` keeps the platform tables (`event_log`, `query_log`, `script_migration`, `task`) behind a third impl in `storage.rs` — the platform owns its own access log and migration bookkeeping, per CONTEXT.md:82-85. The partition is: knowledge `entity, fact, edge, community, claim, claim_job, claim_key_alias, claim_policy, claim_relation, triple, entity_extraction_projection, event_projection_job, procedure_candidate, memory_capture_audit`; memory `episode, inbox_revision, memory_event`; storage `event_log, query_log, script_migration, task`. That is 14 + 3 + 4 = 21, and the test will tell you the 2 missing names — the audit's 23-table count includes tables whose owner this task must place. Do not guess; let the equality assertion name them.
- [ ] **Step 4: Repoint the 4 `select_table` call sites** to pass an `OwnedTable` instead of a `&str`. The three `knowledge` ones (`graph_store.rs:41` `"entity"`, `:62` `"community"`, `knowledge_store.rs:152` `"fact"`) and the `memory` one (`episode_context_store.rs:42` `"episode"`). Each becomes `KnowledgeTables::table("entity")` or `MemoryTables::table("episode")`.
- [ ] **Step 5: Delete `validate_table_name` and `ALLOWED_TABLES`.**
- [ ] **Step 6: Run the test.** Expected: PASS, with every schema table owned exactly once. If `every_expected_schema_table_is_selectable` fails on a table no owner claims, that is the 13-table gap made visible — place the table with its real owner rather than adding it to `storage`'s list to make the test green.
- [ ] **Step 7: Commit**

```bash
git commit -m "fix(storage): one allowlist owner per table, closing the 13-table gap"
```

**DoD:** the equality test passes with 23 tables and no duplicate owner; `select_table` takes an `OwnedTable`; `validate_table_name` and `ALLOWED_TABLES` are gone; a `debug_assert` names the offending table when a context reaches for one it does not own.

## Task 4.2: Return the domain query builders to their owners

`storage/queries.rs` holds SQL for `fact`, `edge`, `community`, and `episode`, plus `BI_TEMPORAL_WHERE` — the bi-temporal visibility predicate that is ADR-0002's central invariant — and `temporal_field_names_for_table` (463-505), which switches on 13 domain tables. The knowledge store delegates its own SQL to the platform at `knowledge_store.rs:46`.

**Files:**
- Create: `crates/memory-mcp/src/knowledge/queries.rs`, `crates/memory-mcp/src/memory/queries.rs`
- Modify: `crates/memory-mcp/src/storage/queries.rs` (delete the moved builders)
- Modify: `crates/memory-mcp/src/storage.rs` (remove the 12 query re-exports at lines 57-73)
- Modify: `crates/memory-mcp/src/knowledge/{knowledge_store,graph_store}.rs`, `src/memory/episode_context_store.rs`, `src/memory/lifecycle_workers/communities.rs`, `src/knowledge/fact_store.rs`
- Modify: `crates/memory-mcp/src/storage.rs` (update the module doc comment at lines 1-21, which currently claims "Nothing here owns domain data" while `queries` and `migrations` both do)

**Interfaces:**
- Consumes: `storage::GraphDirection` (already `pub` at `storage.rs:74`).
- Produces: `BI_TEMPORAL_WHERE` moves to `shared/` — it is a domain invariant that three modules import, not a platform detail:

```rust
// shared/temporal.rs
/// The bi-temporal visibility clause: a row is visible when its validity
/// interval contains the cutoff and its invalidation is not yet ingested.
/// ADR-0002 defines this; every query over a bi-temporal table uses it.
pub const BI_TEMPORAL_WHERE: &str = "...";
```

`temporal_field_names_for_table` splits the same way: the `fact`, `edge`, `episode`, `community` arms go to their owners; `claim`, `claim_job`, `claim_relation` go to `knowledge/claims.rs`; `embedding_state`, `embedding_job` to `embedding/`; `inbox_revision` to `memory/`; `event_log`, `task`, `script_migration` stay in the platform. The dispatch itself becomes a per-owner call rather than a string switch.

- [ ] **Step 1: Write the failing test** in `crates/memory-mcp/tests/knowledge_read_scopes.rs`:

```rust
#[test]
fn the_storage_platform_names_no_domain_table() {
    // Scan crates/memory-mcp/src/storage/ for each of the 19 domain table
    // names (everything in EXPECTED_SCHEMA_TABLES except event_log,
    // query_log, script_migration, task). Assert no hit in a string literal
    // or an identifier.
}
```

Run: expect FAIL — `queries.rs` names 13 of them and `client.rs:939` named 10 (now deleted by Task 4.1).

- [ ] **Step 2: Move `BI_TEMPORAL_WHERE` and `build_fact_visibility_clause`** to `shared/temporal.rs`. Repoint the importers: `knowledge/knowledge_store.rs:17,83`, `knowledge/graph_store.rs:169`, and the fact/edge builders.
- [ ] **Step 3: Move the fact and graph builders** to `knowledge/queries.rs`: `build_select_facts_filtered_query` (228), `build_select_facts_by_entity_links_query` (292), `build_select_facts_ann_query` (309), `build_select_active_facts_query` (338), `build_select_edges_filtered_page_query` (376), `build_select_communities_by_member_entities_query` (389), `build_select_edge_neighbors_query` (398), `build_relate_edge_query` (418), `surreal_string_literal` (269), `active_edge_scan_batch_size` (25).

  `active_edge_scan_batch_size` is called from `memory/lifecycle_workers/communities.rs:123` — a memory module reading a knowledge constant. Make it `pub` in `knowledge/queries.rs` and import it there. That is a memory→knowledge edge, which CONTEXT.md's dependency direction permits; the alternative, a third copy in memory, is worse.
- [ ] **Step 4: Move `build_select_episodes_by_content_query`** (346) to `memory/queries.rs`.
- [ ] **Step 5: Split `temporal_field_names_for_table`** as described. Repoint `build_set_assignments` (507) to take the field list from the caller.
- [ ] **Step 6: Delete the moved code** from `storage/queries.rs` and the re-exports from `storage.rs:57-73`. What remains: `build_select_one_query`, `build_create_query`, `build_update_query`, `build_upsert_query`, `validate_record_id`, `build_create_query` — all table-generic, which is genuinely the platform's job.
- [ ] **Step 7: Move the 21 tests** in `storage/queries.rs:587-782` to the new modules, split by which builder they cover.
- [ ] **Step 8: Update the `storage.rs` module doc** to say what the module now actually contains.
- [ ] **Step 9: Run the test.** Expected: PASS. Then the full suite — this touches a lot of query construction, and a mistake shows up as a failing retrieval test.
- [ ] **Step 10: Commit**

```bash
git commit -m "refactor(storage): the platform keeps the connection, the domains keep their SQL"
```

**Wave 4 DoD:** `storage/` names no domain table and holds no domain SQL; the allowlist has 23 tables with no duplicate owner; the 21 query-builder tests moved with their builders. Gate: full suite, clippy, fmt.

**Wave 4 review:** the specific risk is a query builder that changed behaviour in transit. The reviewer must diff each moved function's body against its original, not accept the move on sight, because a `cutoff` binding or an index hint silently altered in transit will pass every unit test and fail only in production.

---

# Wave 5 — Composition root and extractor interface

## Task 5.1: Record ADR-0067 and move the stdio composition root

`new_from_env_with_mode_and_progress` (`service/core/builder.rs:196-474`) is 278 lines of startup: read config, connect, probe version, apply migrations, resolve the embedding startup decision, construct the provider, construct the NER extractor, write bootstrap-ready state, check connectivity, schedule claim backfill, spawn lifecycle workers, spawn embedding recovery. `bootstrap/` is currently `cfg(control-plane)` and holds only HTTP integration adapters.

**Files:**
- Create: `crates/memory-mcp/src/bootstrap/stdio.rs`
- Modify: `crates/memory-mcp/src/bootstrap.rs` (make the module unconditional; it currently sits behind `#[cfg(feature = "control-plane")]` at line 6)
- Modify: `crates/memory-mcp/src/service/core/builder.rs` (delete lines 196-474; `new_from_env` and `new_from_env_with_mode` move with them)
- Modify: `crates/memory-mcp/src/cli/runtime.rs:60` (`build_memory_service`) and `src/runner.rs`
- Test: `crates/memory-mcp/tests/zero_config_embedded.rs`

**Interfaces:**
- Consumes: everything `builder.rs:196-474` already calls. Nothing changes name.
- Produces:

```rust
// bootstrap/stdio.rs
pub async fn build_memory_service_from_env(
    mode: EmbeddingActivationMode,
    progress: Arc<dyn ModelProgressSink>,
) -> Result<MemoryService, MemoryError>;
```

`MemoryService::new`, `new_with_embedding_provider`, and the `with_*` builder methods stay in `service/core/builder.rs` — they construct the type, which is the container's job. Only the environment-reading orchestration moves.

- [ ] **Step 1: Write ADR-0067.** Status `Accepted`. Context: the container file holds startup policy; `bootstrap/` is HTTP-only and feature-gated. Decision: `bootstrap/` is the composition root for both profiles; the container constructs and holds, it does not start. Consequences: `bootstrap.rs` is no longer `cfg(control-plane)`, so every profile compiles it; `service/core/builder.rs` drops from 496 to ~200 lines. Alternatives considered: (a) keep a `service/startup.rs` — rejected, because it leaves a second place that knows how to build a service, and that is the confusion this change exists to remove; (b) move it into `runner.rs` — rejected, `main.rs` and `runner.rs` must stay thin per AGENTS.md.
- [ ] **Step 2: Write the failing test** in `crates/memory-mcp/tests/zero_config_embedded.rs`:

```rust
#[tokio::test]
async fn zero_configuration_starts_without_any_environment_variable() {
    // Clear SURREALDB_*, call bootstrap::stdio::build_memory_service_from_env,
    // assert Ok, assert the active namespace is "main" and the database "memory".
    // Then assert service.get_active_namespace() == "main".
}
```

There is an existing zero-config test; read it first and extend it rather than duplicating. Run: expect FAIL — the function does not exist.

- [ ] **Step 3: Move the function.** Cut `builder.rs:196-474` and paste into `bootstrap/stdio.rs`, renaming the function. The three `EmbeddingActivationMode` arms at 272-342 move with it, as do the `ner_progress` and `CliProgressSink` wiring. Un-gate `bootstrap.rs:6`.
- [ ] **Step 4: Repoint the two callers.** `cli/runtime.rs:60` `build_memory_service` and `runner.rs`. The local `serve` path in `runner.rs` also builds a service; find it and repoint it too.
- [ ] **Step 5: Run the test.** Expected: PASS, and the existing zero-config, fs-watch, and lifecycle tests must all still pass — they exercise this path.
- [ ] **Step 6: Commit**

```bash
git commit -m "refactor(bootstrap): the stdio composition root leaves the container"
```

## Task 5.2: Remove the container's store-constructor methods

Four `impl MemoryService` methods are pure store constructors: `knowledge_graph_store` (34), `reembed_store` (44), `event_log_store` (53), `episode_store` (80) — each body is `X::new(self.db_client.clone(), self.active_namespace.clone())`. `knowledge_graph_store` has 20+ call sites.

**Files:**
- Modify: `crates/memory-mcp/src/service/core.rs` (delete the 4 methods)
- Modify: every caller: `memory/lifecycle.rs:32,43`, `memory/retrieval/graph_reads.rs:121,150,209`, `memory/retrieval/graph_surprising.rs:50,92,197`, `memory/capabilities/deps.rs:150`, `memory/episode/communities.rs:84,93`, `memory/lifecycle_workers/{archival.rs:148,decay.rs:108,communities.rs:161,167,182}`, `service/apps/dispatch.rs:649,683`, `service/apps/graph.rs:25,156,211,222`, `mcp/handlers/apps.rs:261,330`

**Interfaces:**
- Consumes: `storage::{DbClient, BoundDbClient}` and each store's `::new`.
- Produces: one private helper on the container, so the 24 call sites stay one line each:

```rust
impl MemoryService {
    /// Stores are constructed per call because each binds the Active
    /// Namespace at construction; they are cheap and hold no state.
    pub(crate) fn knowledge_graph_store(&self) -> KnowledgeGraphStore { /* unchanged body */ }
}
```

This task is deliberately small. The container's interface does not shrink here — the store constructors move to where they are called, but the methods stay, because 24 call sites across four bounded contexts need a short path to a store bound to the Active Namespace, and inventing a dependency-injection mechanism for them would be a larger change than the problem warrants (KISS).

- [ ] **Step 1: Confirm with a measurement before acting.** Run `grep -rn "knowledge_graph_store()\|reembed_store()\|event_log_store()\|episode_store()" crates/memory-mcp/src | grep -v "src/service/core.rs" | wc -l`. If the count is under 20, this task is not worth a wave entry — record the finding in the Wave 5 commit message and stop. If it is 20 or more, continue.

  Expected: 24+ (the audit counted 20+ for `knowledge_graph_store` alone). Either way, record the number in the commit.
- [ ] **Step 2: Add a `#[cfg(test)]` test** in `core.rs` that each of the four constructors returns a store bound to `self.active_namespace`, by calling the store's own read method against a seeded namespace. The existing tests at `core.rs:438+` partly cover this; extend rather than duplicate.
- [ ] **Step 3: Verify the test passes before and after.** If it passes before, it is a characterisation test, and that is its purpose: it pins the behaviour so the move cannot break it.
- [ ] **Step 4: Commit** — or, per Step 1, record the measurement and skip.

## Task 5.3: Move the reachable graph traversal out of the container

`service/apps/graph.rs` is 644 lines, roughly 55% of it graph algorithm: the app-session BFS (314-511) and, separately, an introduction-chain BFS (105-243), both traversing the `edge` and `entity` tables through `KnowledgeGraphStore`. ADR-0060 already moved reads out to `memory/retrieval/graph_reads.rs` but left traversal behind.

**Read this before Step 1 — the two halves of this file have different fates.** Verified by tracing every `AppCommandDescriptor` in `service/apps/dispatch.rs`: the `graph` app registers exactly three actions, `expand_neighbors`, `open_edge_details` and `use_path_as_context`. None of them reaches the introduction chain.

- **Reachable, so it moves:** `edge_neighbor` (314), `entity_snapshot` (323), `graph_path_snapshot` (343), `graph_neighbor_expansion` (416), `graph_payload` (477), the `GraphPathSnapshot`/`GraphSessionState` types (250-311). `graph_payload` is called from `mcp/handlers/apps.rs:331,451`; `graph_neighbor_expansion` also from `service/apps/dispatch.rs:648`.
- **Unreachable, so it is cut, not moved:** `find_intro_chain` (131), `intro_chain_from_start` (105), `find_entity_id_by_name` (203), and the `MemoryService::find_intro_chain` wrapper (37-44). The only callers are five sites in `tests/service_acceptance.rs`. `MemoryService::resolve_entity` (50) is a different matter — `memory/capabilities/resolve.rs:17` reaches `memory::api::resolve_entity`, not this method, so the container's copy is also test-only and is handled in Task 5.4.

Moving dead code to a new module would be the worst outcome: it would relocate 139 lines of unreachable BFS and give it a fresh, respectable-looking home in a bounded context. Cutting it is smaller, and it is what the "no dangling functionality" goal actually asks for. Record the deletion and its zero-caller proof in the commit message.

**Files:**
- Create: `crates/memory-mcp/src/knowledge/graph_traversal.rs`
- Modify: `crates/memory-mcp/src/service/apps/graph.rs` (move the reachable BFS out; delete the introduction chain; keep the `GraphContext` port impl)
- Modify: `crates/memory-mcp/src/mcp/handlers/apps.rs:331,451`
- Modify: `crates/memory-mcp/src/service/apps/dispatch.rs:648`
- Modify: `crates/memory-mcp/tests/service_acceptance.rs` (delete the five `find_intro_chain` call sites)
- Test: `crates/memory-mcp/tests/knowledge_read_scopes.rs`

**Interfaces:**
- Consumes: `KnowledgeGraphStore::{select_edge_neighbors, select_entity, relate_edge}`.
- Produces: the four reachable BFS functions and the two snapshot types, moved verbatim, parameterised on `&KnowledgeGraphStore` rather than on `&impl GraphContext` — the `GraphContext` port existed only so the container could supply the store, and after the move the store is passed directly. `GraphContext` stays for the two `impl` blocks that are still needed. `find_entity_id_by_name` is **not** in this list: it is cut with the introduction chain.

- [ ] **Step 1: Write the test that fails when a container function is unreachable.** Extend `crates/memory-mcp/tests/public_surface_audit.rs` (created in Task 1.1) with a table of `(module, function, reachable_via)`. For the introduction chain the row asserts it is *absent* — so the test passes now and fails if anyone re-adds it. This test is the evidence for the cut, and it is what the commit message cites.
- [ ] **Step 2: Move the four reachable functions and the two types** to `knowledge/graph_traversal.rs`. Run the existing graph tests after the move; they must pass unchanged. If a test exists only for a moved function and asserts nothing about the graph, delete it with the function.
- [ ] **Step 3: Delete the introduction chain** — `find_intro_chain` (131), `intro_chain_from_start` (105), `find_entity_id_by_name` (203), the container wrapper (37-44) — and their five call sites in `tests/service_acceptance.rs`. `grep -rn "find_intro_chain\|intro_chain_from_start\|find_entity_id_by_name" crates/` must return nothing.
- [ ] **Step 4: Give `relate` a real caller and real parameters** (Review Focus item 4). `KnowledgeGraphStore::relate_edge` (graph_store.rs:217) is the production path and takes origin, strength, confidence and provenance as parameters. `service/apps/graph.rs:79-82` hardcodes `Inferred`, `1.0`, `0.8`, `manual()`. Change the signature to:

```rust
pub async fn relate(
    &self, from_id: &str, relation: &str, to_id: &str, edge: EdgeAttributes,
) -> Result<(), MemoryError>
```

where `EdgeAttributes { origin: EdgeOrigin, strength: f32, confidence: f32, provenance: Provenance }` is declared in `models/`. The ~30 test call sites and the two `eval-harness` sites pass explicit values. That is a deliberate signature change on a `pub` method: the hardcoded defaults were a business decision hiding in a fixture helper, and no production caller should inherit them. Note that `relate` itself is only called from tests and the eval harness — Task 5.4 decides its fate; this step only stops it from lying about provenance.
- [ ] **Step 5: Write the test** from Review Focus item 4 in `tests/knowledge_read_scopes.rs`: `relate_records_an_operator_originated_edge` — call `relate` with `EdgeOrigin::Operator` and a non-default confidence, read the edge back, assert all four attributes round-trip. Before Step 4 this test cannot be written, because there is no way to pass a non-default value. That is the proof the hardcoding was a real constraint.
- [ ] **Step 6: Commit**

```bash
git commit -m "refactor(knowledge): move the reachable graph traversal, and cut the introduction chain no action reaches"
```

## Task 5.4: Delete the methods the container has only for tests

`find_intro_chain` (37), `resolve_entity` (50), and `episode_count` (153) have zero production callers — every call site is a test. Ruling: these are fixture conveniences on the crate's most public type.

`find_intro_chain` is not in this group: after Task 5.3 it is the moved function, and the `mcp/handlers/apps.rs` call sites are the real consumers. Only `resolve_entity` and `episode_count` qualify.

- [ ] **Step 1: Write the failing test** in `crates/memory-mcp/tests/public_surface_audit.rs`: extend the ratchet from Task 1.1 to assert that every `pub` method on `MemoryService` has at least one non-test caller. Expect FAIL listing `resolve_entity`, `episode_count`, and anything else the audit surfaced.
- [ ] **Step 2: For each method the test flags, choose honestly.** If a test genuinely needs the convenience, move it into the test file as a free function over the same store — `tests/explain_provenance.rs` and `tests/embedded_invalidate.rs` can each have their own, and a duplicated three-line test helper costs less than a production method with no production caller (KISS, and the duplication is in test code where it is visible and cheap).
- [ ] **Step 3: Run the test.** Expected: PASS.
- [ ] **Step 4: Commit**

```bash
git commit -m "refactor(service): a container method with no production caller moves into the test that wanted it"
```

## Task 5.5: Record the Wave 5 state in CONTEXT.md

**Files:**
- Modify: `CONTEXT.md` (module seams section, lines 25-104)

**Interfaces:**
- Consumes: everything Waves 3-5 changed.
- Produces: a CONTEXT.md that matches the tree, which the doc-claim guard in Task 0.3 will keep matching.

- [ ] **Step 1: Update the module seam list.** `src/storage/` — the entry at lines 82-85 now describes what the module contains after Task 4.2. `src/bootstrap/` — add an entry; CONTEXT.md has none today, and after Task 5.1 it is the composition root for both profiles. `src/operations/api.rs` — add the quota policy to the existing entry (line 56). `src/provisioning/api.rs` — add the transition table to the existing entry (line 54).
- [ ] **Step 2: Add the new vocabulary terms** that the changes introduced, in the glossary's existing style — a bold term, a one-to-two-sentence definition, and an `_Avoid_:` line. Candidates: **Quota Admission** (the decision to admit or deny one ingest against a plan and counter; `_Avoid_: rate limit, quota check`), **Tenant Lifecycle Transition** (a legal move between Tenant statuses; `_Avoid_: status update, state change`), **Owned Table** (a table name released only by the bounded context that owns it; `_Avoid_: allowed table, table allowlist`).
- [ ] **Step 3: Update the constraints section** if any changed. None should have: the eight-tool surface, no-`unwrap`, bi-temporal, one-Active-Namespace all still hold.
- [ ] **Step 4: Run the doc-claim guard.** `cargo test -p memory_mcp --test doc_claims`. Expected: PASS.
- [ ] **Step 5: Commit**

```bash
git commit -m "docs(context): record the seams and vocabulary the audit produced"
```

## Task 5.6: Record ADR-0068 and narrow the Entity Extractor interface

ADR-0068 is written here. `ExtractorFingerprint` (`entity_extraction.rs:162-178`) carries `embedding::model_artifacts::{RevisionStatus, ValidationStatus}` and `effective_device` — model architecture in the capability's public interface, which CONTEXT.md:109 says the seam does not expose. Separately, 38 model-artifact references sit inside `knowledge/`, including the candidate-versus-known-good promotion state machine (`gliner.rs:1668-1709`) and safetensors metadata inference (`:417,447,563`).

**Files:**
- Create: `docs/adr/0068-extractor-fingerprint-is-an-opaque-revision-token.md`
- Modify: `crates/memory-mcp/src/knowledge/entity_extraction.rs` (the fingerprint type)
- Modify: the 7 adapter impls, and the 2 tests outside the crate that construct a fingerprint
- Test: `crates/memory-mcp/tests/ner_model_lifecycle.rs`, `crates/memory-mcp/tests/ner_gliner_real_activation.rs`

**Interfaces:**
- Consumes: `embedding::model_artifacts::{RevisionStatus, ValidationStatus}` for the conversion.
- Produces:

```rust
// knowledge/entity_extraction.rs
/// Identifies the Model Checkpoint an extractor is running, as an opaque
/// token. A caller compares tokens for equality and nothing else; the
/// status, the device, and the artifact identity live in the model-artifact
/// module, which is the only place that can interpret them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ExtractorFingerprint(String);

impl ExtractorFingerprint {
    /// The stable token. Two extractors with the same token behave identically.
    pub fn new(token: impl Into<String>) -> Self;
    pub fn as_str(&self) -> &str;
}
```

The adapter's current fingerprint is built by formatting repository, revision, artifact identity and status into a string. That formatting moves into `embedding/model_artifacts` as `pub fn revision_token(&self) -> String`, and each adapter calls it. `knowledge` no longer names a `model_artifacts` type in its public interface.

- [ ] **Step 1: Write ADR-0068.** Status `Accepted`. Context: seven adapters justify the seam; the fingerprint is what leaks through it. Decision: the token is opaque, the format is owned by the model-artifact module. Consequences: any consumer that wanted to branch on `revision_status` must call the model-artifact module instead; a token change is a cache-invalidation event, so the format string is pinned by a test. Alternatives considered: (a) keep the rich fingerprint — rejected, it makes the capability's interface a function of the checkpoint format, which is exactly what CONTEXT.md forbids; (b) move the whole trait into `embedding` — rejected, extraction is a knowledge capability and five backends are not embedding providers.
- [ ] **Step 2: Write the failing test** in `crates/memory-mcp/tests/ner_model_lifecycle.rs`:

```rust
#[test]
fn the_fingerprint_token_format_is_pinned() {
    // Given a known repository, revision and status, assert the token is
    // exactly "<repo>@<revision>:<status>". This pins the format so a change
    // is a deliberate invalidation, not an accident.
}

#[test]
fn knowledge_does_not_name_a_model_artifact_type() {
    // Scan crates/memory-mcp/src/knowledge/ for "model_artifacts::" in any
    // pub signature. Assert none.
}
```

Run: expect FAIL on both.
- [ ] **Step 3: Add `revision_token`** to `embedding/model_artifacts`, with the formatting logic moved out of the adapters.
- [ ] **Step 4: Replace `ExtractorFingerprint`** with the opaque newtype. Update all 7 adapters. Update the 2 out-of-crate consumers — find them with `grep -rn "ExtractorFingerprint" crates/eval-harness crates/memory-mcp/tests`.
- [ ] **Step 5: Run the tests.** Expected: PASS.
- [ ] **Step 6: Commit**

```bash
git commit -m "refactor(knowledge): the extractor fingerprint is an opaque token, not checkpoint state"
```

**Wave 5 DoD:** the composition root is in `bootstrap/`; the reachable graph traversal is in `knowledge/` and the unreachable introduction chain is gone; the container has no test-only public method; CONTEXT.md matches the tree; the fingerprint is opaque. Gate: full suite, clippy, fmt, both guards.

**Wave 5 review:** the reviewer checks that moving the composition root did not change startup order. The specific risk is the embedding startup decision at old `builder.rs:272-342` — a five-arm match whose arm order matters, because a `BootstrapReady` decision must be evaluated after the version probe and before the provider is built. The reviewer must verify each arm still sees the same state it saw before the move. Second risk: the reviewer confirms `grep -rn "find_intro_chain" crates/` returns nothing, since relocating unreachable code into a bounded context would satisfy the letter of "move the traversal" while making the problem worse.

---

# Final verification

Run after Wave 5, before requesting the whole-branch review.

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets \
  --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
cargo test --workspace --lib --bins --tests --locked
cargo test -p memory_mcp --doc --locked
cargo test -p xtask --locked
cargo test -p memory_mcp --test source_tree_integrity --locked
cargo test -p memory_mcp --test doc_claims --locked
cargo test -p memory_mcp --test public_surface_audit --locked
cargo test -p memory_mcp --test no_duplicate_implementations --locked
cargo test -p memory_mcp --lib retrieval --locked
cargo test -p memory_mcp --lib mock_db --locked
cargo test -p memory_mcp --test trait_methods_are_called --locked
cargo run -p xtask -- check-observability
git status --porcelain   # expect empty
```

## Whole-branch review checklist

A fresh reviewer reads the full diff from the audit commit to HEAD. This is the only pass that can see a decision that was right in isolation and wrong in combination.

- [ ] **The nine candidates are all closed.** For each, name the commit that closed it.
- [ ] **No ADR was written for a non-decision.** Five ADRs, and each one answers all three of: hard to reverse, surprising without context, real trade-off.
- [ ] **Every ADR's claim is true of the code.** Read ADR-0065 through 0069 and check each assertion against the tree.
- [ ] **The guards are not theatre.** Break each invariant deliberately and confirm the corresponding test fails. A guard that has never failed is a comment.
- [ ] **Wiring did not become a fake.** For each of the five items wired in Wave 1 (observability, `accelerate`, `mimalloc`, `ner_metal`, the MockDbClient builders), confirm the consumer is real: CI runs it, or a test calls it. A CI step that runs a checker which always passes is worse than no step.
- [ ] **Nothing was deleted that should have been wired.** Re-read the audit's candidate 1 list. Each of the 32 re-exports, 4 engine accessors, `get_surrealdb_config`, 2 test helpers, `update_progress_fenced`, and the 139-line introduction chain — confirm each has a recorded reason for being cut, not just cut.
- [ ] **The test suite did not shrink silently.** Compare `cargo test --workspace -- --list | wc -l` against the count before Wave 0. A drop means coverage was traded for cleanliness. Task 1.6 is the one task that deletes test code, so check its count specifically.
- [ ] **CONTEXT.md is a glossary.** Read it as someone who has never seen the codebase. Any implementation detail, file path that is not a seam, or ADR reference is out of place.
- [ ] **The plan is honest about what it did not do.** The Ruling in Task 3's header says the store traits are not narrowed. Confirm they were not narrowed anyway, and that the plan says so. Likewise Task 1.4's two deliberate non-merges and Task 5.2's measurement-first abort.

---

## Execution handoff

Saved to `docs/superpowers/plans/2026-09-30-architecture-audit-remediation.md` at execution time.

Recommend **subagent-driven execution**. Six waves, twenty-four tasks, and Tasks 4.1, 4.2, 5.3 and 5.6 all touch query construction or the extractor trait — a mistake in any of them passes its own unit tests and fails only in the wider suite. A fresh subagent per task with a fresh reviewer per task, plus the six wave reviews and the branch review, is what catches that. The plan carries the design, so a mid-tier session model is sufficient for the implementation itself.

**Self-review — two passes.**

*First pass, against the spec.* Coverage: all nine candidates have tasks; all Review Focus items have a named test in the task that owns the code. Step scan: no step carries a function body the signature and tests already determine. Type consistency: every name is declared before it is used.

*Second pass, in response to "are you 100% sure?".* The first pass checked the plan against itself. The second went back to the code for every load-bearing claim, and it was right to: **six claims were wrong**, five of them because the first draft generalised from a grep result without reading the code.

| # | Wrong claim | What the code says | Fix |
|---|---|---|---|
| 1 | "Collapse `server_version`'s and `sql_query_take`'s three identical arms onto `run_query_take`." | The arms are identical in *body*, not in *type*. `DbEngine::Local`/`Mem` are `Arc<Surreal<Db>>`, `Remote` is `Arc<Surreal<Client>>`. Collapsing them needs `&Surreal<impl Connection>`, which Rust cannot return without boxing, and SurrealDB's `query` needs the concrete `Connection`. The three-arm match is idiomatic. | Task 1.1 Step 7 now cuts only the 4 accessors and explains why the matches stay. |
| 2 | "Move the introduction-chain BFS to `knowledge/`." | It is reachable from **no** production path. The `graph` app registers exactly three actions (`dispatch.rs:148-162`) and none reaches it; the only callers are five sites in `tests/service_acceptance.rs`. | Task 5.3 splits the file: the four reachable functions move, the introduction chain is cut with a test asserting it stays gone. |
| 3 | "`control/operator.rs` lets a suspended tenant be resumed into a forbidden state." | Wrong mechanism, and it understates the problem. `resume_tenant` never reads `tenant.status`, so it can set **any** status to `Ready`; `suspend_tenant` accepts `Migrating -> Suspended`. `retry_tenant` is already correct. | Review Focus item 2 and Task 3.4 now describe all three handlers precisely, with a four-case test. |
| 4 | "Unify the two `apply_nms` into one function over a shared type." | The duplication is in the *type*, not the function: `ScoredSpan` (private) and `ScoredEntity` (`pub`) are field-for-field identical. | Task 1.4 Step 3 now unifies the type first and points both call sites at the existing `pub(crate) decode::apply_nms`, with a documented fallback. |
| 5 | "Replace `split_whitespace()` in `procedures_service/ranking.rs:72` with `search_query_terms`." | They are different operations. `search_query_terms` lowercases and filters; the ranker wants raw tokens. The change would silently alter which candidates match. | Task 1.4 Step 5 records it as a deliberate non-merge with a comment, not a change. |
| 6 | "`OwnedTable::new` as a `pub const fn`." | A `const fn` cannot look anything up, so it validates nothing — the `ALLOWED_TABLES` problem in a type-safe costume. | Task 4.1 now uses a `pub(crate)` field plus a `ReleaseOwnedTable` trait with a `debug_assert`, and says plainly that the test is the real gate. |

Two more findings the second pass surfaced, both additions rather than corrections:

- **The MockDbClient promise was never scheduled.** I told the user in the audit round that the unused builders would get real consumers; no task did it. Task 1.6 now does, and it is honest about the limit — `MockDbClient` has no `query` seam, so the task adds one, migrates the six fakes that fit, and leaves `CommunityLookupDbClient` in place because it is the only test that asserts call *counts*, which a stateless responder cannot express. `expect_migration_result` is cut rather than left dangling.
- **ADR-0069.** The path-prefix deployment is fully implemented and has no ADR at all. Task 0.3 now writes one, and the ADR count is five.

*Proportion.* The plan is longer than the spec, and the excess is file:line call-site lists, test bodies, and the reasoning behind the six corrections above — the parts an implementer cannot derive. The derivable parts are signatures, not transcripts.
