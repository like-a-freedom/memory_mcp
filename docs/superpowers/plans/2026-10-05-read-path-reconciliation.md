# Read-Path Reconciliation Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [x]`) syntax for tracking.

**Goal:** Make the claim reconciliation layer observable to `assemble_context`, so a superseded fact reaches a reader ranked below its successor and annotated with the relation that demoted it.

**Architecture:** A new `select_relations_by_fact` query in the `knowledge` context maps claims to their source facts and returns relations grouped by fact id. `memory/retrieval` consumes that narrow result through a new `RelationReadPort` declared in `knowledge/api.rs` (Task 3 Step 3), builds `ClaimReconciliationMetadata`, and applies a post-assembly reordering via the pure `demote_superseded` in `ranking.rs`: a fact is demoted strictly below its successor only when the successor is present in the same pack. No scoring constant is introduced.

**Tech Stack:** Rust 1.99.0, SurrealDB (embedded + remote), `tokio`, `async-trait`, `serde`/`schemars`, `thiserror`. Crate: `memory_mcp`.

**Spec:** `docs/superpowers/specs/2026-10-05-memory-quality-evidence-review.md` (Phase 0)

**Status: implemented.** All six tasks landed in commits `d351158`…`5be02dc` (2026-10-05);
the checkboxes are recorded state, not an invitation. Four steps departed from the wording
above and each departure is recorded in its commit body rather than rewritten here, so the
plan still shows what was asked: Tasks 2+3 share a commit (a query with no consumer fails
`-D warnings`, so the per-task gate could not be green independently); Task 5 kept the old
test body under an honest name instead of deleting it; Task 6's measurement came back
unchanged with the reason recorded in its own step; and the rollout gate in Global
Constraints was added during implementation after `CLAIM_RECONCILIATION.md` contradicted
ADR-0074. The gates are green: `cargo fmt --all --check`, `cargo clippy` with `-D warnings`,
3165 `memory_mcp` tests and 203 `eval-harness` tests.
**ADRs:** `docs/adr/0074-connect-the-reconciliation-relation-to-the-read-path.md` (this plan's governing decision), `docs/adr/0058-bounded-contexts-modular-monolith.md` (knowledge owns claims and relations), `docs/adr/0044-narrow-stores-expose-named-methods-only.md`, `docs/adr/0066-business-policy-in-the-owning-context.md`, `docs/adr/0009-separate-claim-supersession-from-fact-retraction.md`, `docs/adr/0022-compact-response-default-for-llm-consumers.md`, `docs/adr/0073-test-behavior-not-document-inventory.md`

## Global Constraints

- Rust toolchain is pinned by `rust-toolchain.toml` to `1.99.0`. `Dockerfile` and the CI setup action must name that same channel or `cargo run -p xtask -- check-toolchain-pin` fails. Do not touch either.
- **The crate's package name is `memory_mcp`, with an underscore.** Every cargo command in this plan uses `-p memory_mcp`. `-p memory-mcp` fails with `package ID specification 'memory-mcp' did not match any packages` — cargo prints a "did you mean" hint, but it is a hard error. This is what CI uses (`.github/workflows/ci.yml:87`).
- **Match CI's test invocation** so a locally green run means what a CI run means:
  ```bash
  cargo test -p memory_mcp --lib --bins --tests \
    --features fs-watch,mcp-apps,streamable-http,test-fixtures --locked
  ```
  `test-fixtures` is not optional here — it is in CI's clippy and test feature sets (`.github/workflows/ci.yml:54,87,98`). A narrower local command can pass while CI fails.
- Lint gate, zero warnings tolerated:
  ```bash
  cargo clippy --workspace --all-targets \
    --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
  ```
- Format gate: `cargo fmt --all --check` must produce zero diff.
- `main.rs` stays thin: CLI parsing and mode dispatch only. No business logic there.
- A use case never receives the whole `MemoryService`. Inject the narrow port it needs.
- Raw SurrealDB queries are never exposed as MCP tools and never owned outside the context that declares the data. Claims and relations belong to `knowledge` (ADR-0058); `memory/retrieval` must not compose claim SQL.
- New business logic does not go in `src/service/`; it goes in the owning context's `api.rs`.
- Facts are never deleted. `invalidate` performs **retraction** (transaction time only), which is the deliberate opposite of **supersession** (closes the earlier claim's validity interval). Never reuse one to represent the other.
- MCP tool surface is frozen at eight tools. No task may add one.
- **No migration in any task of this plan.** `valid_to` is never written and stays that way — see ADR-0074. Adding a migration here would trip `latest_registered_migration_is_expected` (`knowledge/claims.rs:792`) for no reason.
- Per ADR-0073: tests assert observable behavior through a public interface. No test asserts that a file, directory, or doc exists.
- Order of work is load-bearing. Task 1 removes a dead field, so it lands first and alone. Task 3 depends on Task 2's query **and** adds the `superseded_by_fact_id` field Task 4 reads — Task 4 cannot compile before Task 3 lands. Task 6 measures Tasks 3–4 together and must run last.
- **ADR-0074 needs an amendment before Task 3 lands.** Its Consequences section says "`ClaimRelationSummary` already carries `counterpart_fact_id`, so pointing a reader at the replacement needs no schema change." Task 1 removes that field as dead, which is the ADR's own instruction — and then the justification evaporates, because the reordering genuinely needs a successor fact id and none is left. The plan therefore **does** change the schema, by one field.
  This does not contradict the ADR's *decision* — it is still a post-assembly reordering with no constant and no new candidate axis — only its stated rationale, which was written against a field that was never populated. Amend the Consequences clause to say the successor fact id arrives in `superseded_by_fact_id`, added by this plan. Do not silently diverge from a governing ADR; amend it in the Task 1 commit so the record shows the correction and its cause together.
- **Disclosure is gated by the claim rollout stage.** `docs/evals/CLAIM_RECONCILIATION.md:125` states that the default `shadow` stage "projects claims but does not expose relations in `assemble_context`", and ties promotion to `evidence` to precision/recall thresholds. ADR-0074's Decision says an item *must* expose its relations. Both hold: the read path serves relations only at `evidence`. The gate lives in `knowledge::api::SurrealRelationReader`, never in `memory/retrieval`, so a deployment cannot be talked into disclosure from the retrieval layer — and because `demote_superseded` reads the rows the gate withholds, one gate covers the metadata *and* the reordering. Every test asserting disclosure must put its service on `evidence` via `MemoryService::with_claim_rollout_stage`; the evaluation harness does the same, because a run at `shadow` measures the feature's absence and reports it as evidence about the feature.

## Review Focus

Each line is an input class or failure mode that a reasonable engineer would expect to work but that no naive implementation would handle. The test named in parentheses lives in the task listed.

1. **The successor is not in the pack.** A query for "what is my deployment" may return only the old value because the new one fell outside the budget. Demoting the predecessor anyway hands the reader nothing. The predecessor must keep its rank. (Task 4, `leaves_ranking_untouched_when_successor_is_absent`)
2. **The relation's counterpart fact is outside the budget.** Same shape, opposite direction: the predecessor is present and marked superseded, but the successor is absent, so no reader-visible gain is available and no demotion may be invented. (Task 4, `leaves_ranking_untouched_when_successor_is_absent`)
3. **A duplicated fact, not a superseded one.** Duplicates are redundancy. Demoting a duplicate can remove the only surviving copy once decay retires one of the pair. (Task 4, `never_demotes_duplicate_outcome`)
4. **An invalidated relation row.** Relations are versioned and carry `t_invalid_ingested`. A row that has been retracted must not demote anything. (Task 2, `excludes_invalidated_relations`)
5. **A claim whose source fact is not in the result set.** The query is driven by the caller's fact ids, so an unmatched claim must not create a phantom entry or panic on an absent map key. (Task 2, `ignores_relations_for_absent_facts`)
6. **The service is not on the disclosure stage.** `shadow` persists relations but serves none, so a test asserting disclosure would fail for a reason unrelated to the code under test — and an evaluation run at `shadow` would measure the feature's absence and publish it as evidence about the feature. Every disclosure test sets the stage explicitly. (Task 3, `withholds_relations_below_the_evidence_stage`, and Task 6, `reconciliation_changes_which_fact_leads_the_pack`)
7. **The assertion cannot fail.** A ranking test whose query already puts the newer value first passes whether or not the policy ran. This is the failure mode most likely to ship a green test that proves nothing; each ordering assertion must be checked against the policy disabled before it counts as evidence. (Task 4 and Task 6, mutation verification recorded in their commit bodies)

---

### Task 1: Remove the never-populated `counterpart_fact_id`

The field is declared at `models/request.rs:277` and its only occurrence in the tree is that declaration — nothing writes or reads it. `claim_relation.predecessor_claim_id` / `successor_claim_id` already record direction, so this field duplicates a truth that is stored authoritatively. Removing it now, in its own commit, keeps Task 3's diff attributable if anything regresses.

**Files:**
- Modify: `crates/memory-mcp/src/models/request.rs:271-281` (drop the field from `ClaimRelationSummary`)
- Modify: `docs/adr/0074-connect-the-reconciliation-relation-to-the-read-path.md` (amend the stale "no schema change" rationale — Step 4)

**Interfaces:**
- Consumes: nothing.
- Produces: `ClaimRelationSummary { relation_id, outcome, counterpart_source_episode_id, reason_code, evaluator_version }`. Later tasks build on this shape; **Task 3 adds exactly one field to it**, `superseded_by_fact_id: Option<String>`, and changes nothing else.

- [x] **Step 1: Confirm the field is dead before deleting it**

Run:
```bash
grep -rn "counterpart_fact_id" crates/ --include=*.rs
```
Expected: exactly one hit, the declaration at `models/request.rs:277`. **If there is any second hit, stop and report it** — that means a writer or reader exists and this task's premise is false.

- [x] **Step 2: Remove the field**

Delete this line from `ClaimRelationSummary` in `crates/memory-mcp/src/models/request.rs`:
```rust
    pub counterpart_fact_id: String,
```

- [x] **Step 3: Build**

Run: `cargo build`
Expected: compiles. If any constructor sets the field, Step 1 was wrong — stop and report.

- [x] **Step 4: Amend ADR-0074's stale rationale in the same commit**

ADR-0074 justified "no schema change" by pointing at `counterpart_fact_id` — the field this task deletes. Correct that clause so the ADR matches what the plan actually does (Global Constraints): the successor fact id arrives in a new `superseded_by_fact_id` field added by Task 3.

The ADR's decision — post-assembly reordering, no constant, no new candidate axis — stands unchanged. Only the rationale is corrected. Same commit as the deletion, so the record shows both together.

- [x] **Step 5: Verify the full gate set**

Run:
```bash
cargo fmt --all --check && \
cargo clippy --workspace --all-targets \
  --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
```
Expected: both clean.

- [x] **Step 6: Commit**

```bash
git add crates/memory-mcp/src/models/request.rs docs/adr/0074-connect-the-reconciliation-relation-to-the-read-path.md
git commit -m "refactor: drop never-populated counterpart_fact_id from ClaimRelationSummary"
```

---

### Task 2: A narrow relation query owned by `knowledge`

`select_relations_for_facts` exists on the `ClaimStore` trait (`knowledge/claims.rs:53`) but is used only for extraction-time warnings and rollout exposure — two sites in `memory/episode/fact_extraction.rs` (lines 254 and 476) plus one `eval_support.rs:135` behind the `eval-support` feature. Nothing on the retrieval path calls it.

`select_relations_for_facts` filters by **fact**, while the demotion rule needs **claim → fact** direction. `Claim.source_fact_id` (`models/claim.rs:603`) provides it. This task adds a query that returns relations together with the fact each side's claim came from, so the retrieval layer never joins claim data itself.

**Files:**
- Modify: `crates/memory-mcp/src/knowledge/claims.rs` (query struct + trait method + `SurrealClaimStore` impl + `#[cfg(test)] mod tests`)
- Test: `#[cfg(test)] mod tests` **inside `claims.rs`** — see the visibility note below

`eval_support.rs` is deliberately **not** in this list; see Step 6 for why widening it would be an unrequested public-API change.

> **Visibility — read before choosing a test location.** `ClaimStore` (`knowledge/claims.rs:30`) and `SurrealClaimStore` (`claims.rs:151`) are both `pub(crate)`. An integration test under `tests/` is a separate crate and **cannot name either type**, so a test there will not compile. This is why `tests/claim_reconciliation_e2e.rs` drives everything through public capabilities (`IngestCapability`, `ExtractCapability`) and never touches the store.
>
> The project's own testing standard says a real database — including an embedded in-memory engine — is an integration dependency even when the test sits under `#[cfg(test)]`. So: put the query tests in `claims.rs`'s own `#[cfg(test)] mod tests` using an in-memory `DbClient`, which is the only way to reach the type at all, and say so in the commit body. Do not widen `ClaimStore` to `pub` to make a `tests/` file work; that changes the public surface for a test convenience.

**Interfaces:**
- Consumes: `ClaimStore` trait, `Claim.source_fact_id: FactId`, `ClaimRelation.predecessor_claim_id` / `successor_claim_id: Option<ClaimId>`, `ClaimRelationOutcome`.
- Produces:
  ```rust
  pub(crate) struct RelationForFact {
      pub relation_id: String,
      pub outcome: ClaimRelationOutcome,
      pub reason_code: String,
      pub evaluator_version: String,
      pub predecessor_fact_id: Option<FactId>,
      pub successor_fact_id: Option<FactId>,
      pub predecessor_source_episode_id: Option<EpisodeId>,
      pub successor_source_episode_id: Option<EpisodeId>,
  }

  pub(crate) struct RelationsByFactQuery<'a> {
      pub fact_ids: &'a [FactId],
  }

  // on trait ClaimStore:
  async fn select_relations_by_fact(
      &self,
      query: RelationsByFactQuery<'_>,
  ) -> Result<Vec<RelationForFact>, MemoryError>;
  ```
  Direction semantics: `predecessor_fact_id` is the fact whose claim lost; `successor_fact_id` is the fact whose claim replaced it. Both `None` for outcomes with no direction (`Contradiction`, `TemporalAmbiguity`). A relation whose `t_invalid_ingested` is set is omitted entirely.

- [x] **Step 1: Write the failing tests**

Add a `#[cfg(test)] mod tests` to `crates/memory-mcp/src/knowledge/claims.rs`. It already has one — `latest_registered_migration_is_expected` lives at line 792 — so extend that module rather than adding a second. Reuse its in-memory store bootstrap if one exists; `SurrealClaimStore::new(db: Arc<dyn DbClient>, namespace)` (line 156) is the only constructor.

```rust
#[tokio::test]
async fn maps_supersession_direction_to_facts() {
    // Arrange: two claims with distinct source_fact_id, related by a persisted
    // claim_relation row whose predecessor_claim_id/successor_claim_id point at them.
    // Act
    let relations = store
        .select_relations_by_fact(RelationsByFactQuery { fact_ids: &[old_fact, new_fact] })
        .await
        .expect("query succeeds");
    // Assert
    assert_eq!(relations.len(), 1);
    assert_eq!(relations[0].outcome, ClaimRelationOutcome::Supersession);
    assert_eq!(relations[0].predecessor_fact_id, Some(old_fact));
    assert_eq!(relations[0].successor_fact_id, Some(new_fact));
}

#[tokio::test]
async fn excludes_invalidated_relations() { /* t_invalid_ingested = Some(now) on the
    relation row → result is empty */ }

#[tokio::test]
async fn omits_direction_for_undirected_outcomes() { /* outcome = Contradiction →
    both fact ids are None, even though left/right claim ids are set */ }

#[tokio::test]
async fn ignores_relations_for_absent_facts() { /* query with an unrelated fact id
    → result is empty, no error */ }
```

**Fixture note.** `common::seed_fact_at` lives in `tests/common/` and is not reachable from inside the crate; do not try to use it here. This test builds `claim` and `claim_relation` rows directly through the `DbClient`. That is appropriate: the unit under test is a *query*, and `tests/claim_reconciliation_e2e.rs` already covers the full ingest → extract → reconcile pipeline end to end. Do not duplicate that pipeline here.

- [x] **Step 2: Run the test to verify it fails**

Run: `cargo test -p memory_mcp --lib knowledge::claims`
Expected: **compile error** — `RelationsByFactQuery` and `select_relations_by_fact` do not exist yet. This is the correct failure; a runtime assertion failure would mean you are testing existing behavior.

- [x] **Step 3: Add the query types to `knowledge/claims.rs`**

Add `RelationForFact` and `RelationsByFactQuery` next to the existing `RelationsForFactsQuery` (around line 105), and add the `select_relations_by_fact` declaration to the `ClaimStore` trait after `select_relations_for_facts` (line 57).

- [x] **Step 4: Implement the SurrealDB query**

Implement it on `SurrealClaimStore`. Two options; take the first:

- **Take first (recommended):** resolve the caller's fact ids to their claims via the existing `select_claims_for_facts` (line 44), build a `HashMap<ClaimId, FactId>`, then query `claim_relation` rows whose `left_claim_id` or `right_claim_id` is in the resulting claim set. Map each row through the `HashMap` to fill `predecessor_fact_id` / `successor_fact_id`; when either id is absent from the map, leave that `Option` as `None` rather than erroring.
- **Take second (single round trip):** one statement joining `claim_relation` against `claim` twice.

The first is correct and simpler; pick the second only if a query-count assertion in Task 5 requires it.

Direction mapping, exactly: when `outcome` is `Supersession` or `Correction`, resolve `predecessor_claim_id` and `successor_claim_id` through the map. For every other outcome set both to `None` — do not derive direction from `left_claim_id`/`right_claim_id`, which are unordered storage columns.

Filter out rows where `t_invalid_ingested` is present and non-null. Set `predecessor_source_episode_id` / `successor_source_episode_id` from the matching `Claim.source_episode_id`.

- [x] **Step 5: Make every `ClaimStore` implementor compile**

`ClaimStore` is implemented by test doubles too (`knowledge/claims_policy/worker.rs:466`, `claims_policy/projection.rs:406`). Add the new method to each. **Do not** give a double a real implementation it does not need — `todo!()` is acceptable for a double that no test exercising it will call, but the compiler will reject an omitted method.

- [x] **Step 6: Do not widen the public surface**

`RelationForFact` stays `pub(crate)`. `eval_support::ClaimEvidenceReader` already exists as a public seam (`eval_support.rs:100`, used by `crates/eval-harness/src/suites/claims.rs:608`), but **nothing in this plan calls it** — Task 6 measures through the eval harness, not through that reader. So this plan does not touch `eval_support.rs` and does not make `RelationForFact` `pub`.

An earlier draft added `relations_by_fact` to `ClaimEvidenceReader` "so Task 6 can reach persisted relations." Task 6 does no such thing. Shipping it would widen the public API for a caller that does not exist, which is the failure mode YAGNI exists to prevent. If a later phase genuinely needs this seam, that phase adds it — with its own consumer and its own tests.

Corollary: `RelationForFact` is consumed only inside the crate (Task 2's unit tests, Task 3's projection), so `pub(crate)` is correct and sufficient. Do not add `pub use` re-exports to `lib.rs` for any of these types.

- [x] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p memory_mcp --lib knowledge::claims`
Expected: the four added here — `maps_supersession_direction_to_facts`,
`excludes_invalidated_relations`, `omits_direction_for_undirected_outcomes`,
`ignores_relations_for_absent_facts` — pass, alongside the module's existing
tests. The filter is a module prefix, not a test list, so the run reports
considerably more than four.

- [x] **Step 8: Verify the full gate set**

```bash
cargo fmt --all --check && \
cargo clippy --workspace --all-targets \
  --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
```
Expected: clean.

- [x] **Step 9: Commit**

```bash
git add crates/memory-mcp/src/knowledge/claims.rs
git commit -m "feat(knowledge): add select_relations_by_fact mapping relation direction to facts"
```

---

### Task 3: Populate `reconciliation` on the assembled item

`AssembledContextItem.reconciliation: Option<ClaimReconciliationMetadata>` already exists (`models/request.rs:319`) and is always `None` — grep for `reconciliation: Some` returns nothing in the tree. `ClaimReconciliationMetadata` holds `claim_ids: Vec<String>` and `relations: Vec<ClaimRelationSummary>` (`models/request.rs:264-269`).

This task makes the read path populate the field it already declares.

**Files:**
- Create/Modify: `crates/memory-mcp/src/knowledge/api.rs` (declare `RelationReadPort` beside `KnowledgeReadPort`, plus the named projection function)
- Modify: `crates/memory-mcp/src/memory/retrieval_deps.rs` (add the port field)
- Modify: `crates/memory-mcp/src/memory/retrieval.rs` (fetch relations once, after the view dispatch at line 226, before `store_cache` at line 334)
- Modify: `crates/memory-mcp/src/models/request.rs` — **two changes**: add `superseded_by_fact_id: Option<String>` to `ClaimRelationSummary`, and apply the compact gate to `reconciliation`
- Test: `crates/memory-mcp/tests/assemble_context_reconciliation.rs` (new)

**`knowledge/api.rs` is a required file, not optional.** The port lives in the owning context (ADR-0066, Global Constraints), so omitting it here is what led an earlier draft to put a raw `Arc<dyn ClaimStore>` into `retrieval_deps.rs`.

**Do not edit `views.rs`, `scoring.rs`, or `budget.rs`.** `AssembledContextItem` is constructed in production at **twelve sites across three files** — `views.rs` (lines 72, 261, 267, 344, 350, 409, 415, 431, 437), `scoring.rs` (15, 55), `experience.rs` (167) — and each `build_*_view` is a separate `match` arm. The other constructors (`budget.rs`, `rescue.rs`, `logging.rs`, `invalidate.rs`, `agent_memory/recall.rs`) all sit inside `#[cfg(test)]` modules and are irrelevant here. Patching production construction sites means touching twelve places and missing one silently. The single point after the dispatch covers all of them.

**Interfaces:**
- Consumes: `RelationReadPort` from Step 3, whose single method delegates to `ClaimStore::select_relations_by_fact` (Task 2). **`memory/retrieval` never names `ClaimStore`** — Step 3 is explicit that this task depends on the port, not the store.
- Produces: `AssembledContextItem.reconciliation` is `Some(ClaimReconciliationMetadata { .. })` for any item whose fact participates in at least one relation; `None` otherwise. The **decision** this produces — demotion — is Task 4; this task only attaches data.

- [x] **Step 1: Write the failing test**

Create `crates/memory-mcp/tests/assemble_context_reconciliation.rs`:

```rust
mod common;

#[tokio::test]
async fn exposes_supersession_on_the_predecessor_item() {
    // Arrange: two facts whose claims reconcile into a Supersession relation.
    // Drive ingest → extract → claim projection; `seed_fact_at` creates no claim.
    // The request must set `compact: false` — it defaults to true, which omits
    // `reconciliation` from serialization and would make these assertions fail
    // for a reason unrelated to the fix (Step 6).
    //
    // Act
    let items = AssembleContextCapability::assemble_context_from_service(&service, req).await
        .expect("assembles");
    //
    // Assert
    let a = items.iter().find(|i| i.fact_id == a_fact_id).expect("A present");
    let rel = a.reconciliation.as_ref().expect("A carries reconciliation")
        .relations.iter().find(|r| r.outcome == ClaimRelationOutcome::Supersession)
        .expect("A carries the supersession relation");
    assert!(!rel.reason_code.is_empty());
    assert!(!rel.counterpart_source_episode_id.is_empty());
    assert!(!a.reconciliation.as_ref().expect("reconciliation").claim_ids.is_empty());
}

#[tokio::test]
async fn leaves_reconciliation_none_when_no_relation_exists() { /* one fact, no
    relations → item.reconciliation.is_none() */ }
```

`assemble_context_from_service` returns `Vec<AssembledContextItem>` directly (`service/memory_container_shims/memory_capabilities_assemble_context.rs:21-26`) — there is no wrapper struct and no `.items` field.

- [x] **Step 2: Run the test to verify it fails**

Run: `cargo test -p memory_mcp --test assemble_context_reconciliation --features test-fixtures`
Expected: FAIL — `reconciliation` is `None`, exactly the defect this plan fixes. A compile error means you referenced something Task 2 did not produce.

- [x] **Step 3: Add the port in the owning context, not in the retrieval module**

Do **not** add a raw `Arc<dyn ClaimStore>` field to `AssembleContextDeps` and call it from `retrieval.rs`. That was the earlier shape of this step and it is wrong twice over: it hands a storage adapter to a use case (ISP — `assemble_context` would depend on every method of a 10-method trait), and it puts the projection policy in a transport-shaped module rather than the context that owns it (ADR-0066).

`memory/api.rs` already declares the retrieval seam this plan should extend:

```rust
pub trait ContextRetrievalPort: Send + Sync {   // memory/api.rs:88
    async fn retrieve(&self, command: &RecallCommand)
        -> Result<Vec<AssembledContextItem>, MemoryError>;
}
```

`knowledge/api.rs:57` shows the established shape for a knowledge port — `KnowledgeReadPort`, owner-injected, with named use-case functions beside it (`owned_fact_scan`, `owned_episode_scan`). Follow it:

- Declare `RelationReadPort` in `knowledge/api.rs`, next to `KnowledgeReadPort`.
- Declare the projection in `knowledge/api.rs` as a named function, e.g. `relations_for_facts(port, fact_ids) -> Result<Vec<RelationForFact>, MemoryError>`. It already exists in spirit: `select_relations_by_fact` from Task 2 is the implementation behind this port. The port exists so `memory` depends on an interface in the owning context, not on `SurrealClaimStore`.
- `memory/retrieval` consumes `RelationReadPort`, not `ClaimStore`.

One narrow port with one method beats passing the full store. Do not add a cache-bypass or debug flag to any of them (YAGNI).

**Wiring — this is the part that does not compile if it is missed.** `AssembleContextDeps` is built in exactly **one** place: the `From<&MemoryService>` conversion at `service/retrieval_deps_from_container.rs:10`. Every sibling port there is constructed fresh from `ctx.db_client.clone()` plus `ctx.active_namespace.clone()` — `KnowledgeStoreClient::new`, `FactAccessStore::new`, `EpisodeContextStore::new` and so on. Follow that pattern rather than reaching for a field on the service:

```rust
// in the From<&MemoryService> impl
relation_read: Arc::new(SurrealRelationReader::new(
    ctx.db_client.clone(),
    ctx.active_namespace.clone(),
)),
```

`SurrealRelationReader` is the adapter implementing `RelationReadPort`; give it the same `new(db, namespace)` shape as its siblings and delegate to the Task 2 query. Put it in `knowledge/api.rs` beside the trait — that file already holds `read_through_knowledge_port`, the owner-side adapter for `KnowledgeReadPort`, so this is the established home and not a new pattern.

Two things do **not** need changing:
- `service/apps/graph.rs:62` implements `GraphContext for AssembleContextDeps`. Adding a field is fine — a trait impl that never reads the new field still compiles. No change there.
- Nothing else constructs `AssembleContextDeps`; `grep -rn "AssembleContextDeps {" crates/` returns this one file plus the two impl blocks. A second construction site appearing later is a compile error, which is the intended failure.

- [x] **Step 4: Implement the projection**

Insert after the view dispatch (`retrieval.rs:226`, where `results` is bound) and before `store_cache` (`retrieval.rs:334`). Collect the distinct `fact_id`s from `results`, call `select_relations_by_fact` **once** for the whole set — a per-item call is N queries on the hot path — group by fact id, and set `reconciliation` on each item whose fact appears in the group.

Populate `claim_ids` from `select_claims_for_facts` rather than leaving it empty; an always-empty vector is the same defect in a smaller box.

Direction: fill `ClaimRelationSummary` from `RelationForFact`. The summary carries `counterpart_source_episode_id` (Task 1 removed `counterpart_fact_id`), so a reader learns which episode the replacement came from without the retrieval layer re-deriving direction.

**Place this before `store_cache`, not after.** The cache stores `Vec<AssembledContextItem>` (`retrieval_deps.rs:46-51`), so items stored without relations would be served back with `reconciliation: None` forever. Details in the cache section below.

- [x] **Step 5: The context cache will hide your work — read this before writing the test**

Three facts about the cache that change how Tasks 3–6 must be tested:

1. **The cache is always on and cannot be disabled.** `MemoryService::new` hardcodes `cache_size: crate::service::CONTEXT_CACHE_SIZE` = 512 (`service/core/builder.rs:185`, `service.rs:96`). There is no `no_cache` flag, no env var, no builder override. `common::TestMemory::new` (`tests/common/mod.rs:34`) goes through that constructor, so every integration test runs with caching enabled.
2. **A cache hit returns before any post-processing.** `retrieval.rs:163-190` returns `Ok(cached)` immediately. Anything placed after the dispatch — including the Step 4 projection and Task 4's reordering — is **skipped entirely** on a hit.
3. **Writes do not clear it.** `invalidate_cache` exists (`memory/context_cache/invalidation.rs:17`) but is called from exactly one place: `memory/capabilities/invalidate.rs:87`. Ingest does not clear it.

Consequence for every test in Tasks 3, 4, 5 and 6: **a test that assembles the same query twice will get a cache hit on the second call and observe the pre-change result.** Three ways out, in order of preference:

- Assemble each query exactly once per test, and assert on that result. Preferred — no production change, no fragility.
- Vary a parameter that participates in `CacheKey` (`retrieval/pipeline.rs:105-117`: query, cutoff, budget, `fact_types`, view mode, window, allowed tags). Only if a scenario genuinely needs a second assembly.
- Call `memory::context_cache::invalidate_cache` directly between assemblies. It is `pub` within the crate, not exported to integration tests — reaching it from `tests/` may not compile. Use this only after confirming accessibility, and prefer the first option.

Do **not** add a cache-bypass parameter to the public request. `AssembleContextRequest` is a frozen MCP surface; adding a field to it is out of scope for this plan.

- [x] **Step 6: Gate the field under compact mode**

`AssembledContextItem` gates `quote` with `skip_serializing_if = "crate::tools::compact::skip_if_compact"` (`models/request.rs:292`). `reconciliation` currently uses `skip_serializing_if = "Option::is_none"` (line 319).

`compact` **defaults to true** (`request.rs:168-174`, `default = "crate::tools::parsers::default_compact"`). Two consequences, both of which will otherwise look like a broken implementation:

1. **Every test in this plan must set `compact: false`.** Under the default, `reconciliation` is omitted and `items[i].reconciliation` is `None` for reasons that have nothing to do with the fix. The existing tests in `longmem_acceptance.rs` already pass `compact: false` explicitly (line 37) — copy that.
2. **The demotion must not depend on the serialized field.** Task 4 reorders on the in-memory value produced before serialization, so `compact: false` affects only what a reader sees, never the ordering. Verify this holds: with `compact: true` the order must still be demoted while the metadata is absent. That asymmetry is intentional and is the reason for the gate.

ADR-0022 froze compact responses as the default for LLM consumers, and the response-size gate protects that budget — adding a relation vector moves it. Apply the same compact gate `quote` uses, so under `compact=true` the full `relations` vector is omitted. Measure the compact payload before and after rather than inspecting it.

- [x] **Step 7: Run the test to verify it passes**

Run: `cargo test -p memory_mcp --test assemble_context_reconciliation --features test-fixtures`
Expected: **3 passed** — `exposes_supersession_on_the_predecessor_item`,
`leaves_reconciliation_none_when_no_relation_exists`, and
`withholds_relations_below_the_evidence_stage`. Task 6 adds a fourth to this file.

- [x] **Step 8: Run the retrieval and size gates**

```bash
cargo test -p memory_mcp --features fs-watch,mcp-apps,streamable-http
cargo run -p eval-harness --bin memory-eval -- run \
  --profile evals/profiles/release.json \
  --artifact target/eval/gate-check.json \
  --baseline evals/baselines/one-active-namespace-release.json
```

Expected: the first command all green; the second prints one `gate=… status=passed`
line per gate and `RESULT: PASSED`. The response-size and retrieval **gates** (the
ones with floors and a regression budget) are implemented in `eval-harness` —
`crates/eval-harness/src/suites/response_size.rs` and `suites/retrieval.rs` — not
in `memory_mcp`, so running only the first command would run none of them. If the
size gate fails, Step 6 was applied incorrectly — fix the gate, do not raise the
budget. `target/` must exist first; the harness does not create it.

- [x] **Step 9: Commit**

```bash
git add crates/memory-mcp/src/memory/retrieval crates/memory-mcp/src/models/request.rs crates/memory-mcp/tests/assemble_context_reconciliation.rs
git commit -m "feat(retrieval): populate reconciliation metadata on assembled context items"
```

---

### Task 4: Demote a superseded fact below its successor

The decision (ADR-0074): a fact is demoted **strictly below** its successor when both are present in the assembled pack; when no successor is present, it is **not** demoted. `Duplicate` is never demoted — it is redundancy, and demoting it can remove the only surviving copy once decay retires one of the pair.

This is a post-assembly reordering, not a scoring term. Successor presence is unknowable during candidate selection.

**Files:**
- Modify: `crates/memory-mcp/src/memory/retrieval/ranking.rs` (add `DemoteSupersededRequest`, `demote_superseded`, and the unit tests in the existing `mod tests` at line 1230)
- Modify: `crates/memory-mcp/src/memory/retrieval.rs` (one call site, in the block Task 3 added — after the view dispatch at line 226, before `store_cache` at line 334)
- Test: `crates/memory-mcp/tests/assemble_context_supersession_ranking.rs` (new)

`ranking.rs` has exactly one `#[cfg(test)] mod tests` (line 1230). The two other `#[cfg(test)]` items (lines 694, 1200) are test-gated helper functions, not modules — extend line 1230, do not add a second module.

**Interfaces:**
- Consumes: `AssembledContextItem.reconciliation` populated by Task 3.
- Produces: within the returned item list, for every item `x` that has a `Supersession` or `Correction` relation whose successor fact is also present in the list, `index(x) > index(successor)`. Every other relative order is unchanged.

- [x] **Step 1: Write the failing tests**

Two levels, because the policy and the wiring fail differently.

**Policy level — unit tests in `ranking.rs`.** `demote_superseded` is pure, so its three rules are tested with constructed `AssembledContextItem` values and no database. Add them to `ranking.rs`'s existing `#[cfg(test)] mod tests`, one scenario each per the project's testing standard:

```rust
#[tokio::test]  // not async; plain #[test]
fn demotes_predecessor_below_successor() { /* A superseded by B, both present →
    index(B) < index(A) */ }

#[test]
fn leaves_ranking_untouched_when_successor_is_absent() { /* A marked superseded, B
    absent → order unchanged and A still present */ }

#[test]
fn never_demotes_duplicate_outcome() { /* A and C related by Duplicate →
    original relative order preserved, both present */ }
```

**Wiring level — integration tests proving `assemble_context` calls the policy.** Create `crates/memory-mcp/tests/assemble_context_supersession_ranking.rs` with `mod common;` at the top. **Assemble each query exactly once per test** — a second assembly of the same query hits the context cache and returns the pre-reordering list (Task 3 Step 5):

```rust
#[tokio::test]
async fn demotes_predecessor_below_successor_in_an_assembled_pack() {
    // Arrange: fact A superseded by fact B; a query matching both.
    // Act
    let items = AssembleContextCapability::assemble_context_from_service(&service, req).await
        .expect("assembles");
    // Assert
    let rank = |id: &str| items.iter().position(|i| i.fact_id == id).expect("in pack");
    assert!(rank(&b_fact_id) < rank(&a_fact_id), "successor must outrank predecessor");
}
```

Keep the wiring level to **one** test. The three policy rules are already covered by the unit tests above; repeating them through a database proves the wiring, not the policy, and a third copy is the same assertion maintained in two places. The unit test for "successor absent" is where that rule belongs — arranging a budget-excluded successor through the full pipeline is fragile and tests the budget, not the rule.

`never_demotes_duplicate_outcome` asserting only an order is weak evidence — it would pass if the reorder were a no-op. Assert both facts remain present too, so the test detects a demotion that pushed one out of the budget.

- [x] **Step 2: Run the tests to verify they fail**

```bash
cargo test -p memory_mcp --lib memory::retrieval::ranking
cargo test -p memory_mcp --test assemble_context_supersession_ranking --features test-fixtures
```
Expected: the unit tests FAIL (no `demote_superseded` yet) and the wiring test FAILS (nothing demotes A). After the policy exists, both halves must go green — a green wiring test with a missing unit test means the policy was never pinned.

- [x] **Step 3: Implement the reordering as a pure function in `ranking.rs`**

Do not write the loop inline in `retrieval.rs`. `retrieval.rs` is already 1409 lines and is the transport-shaped dispatcher; ADR-0066 puts policy in the owning context as pure functions over state, and `ranking.rs` (2023 lines) is where ranking policy already lives as pure functions taking a `…Request` struct — `build_ranked_context_facts(BuildRankedContextFactsRequest { .. }, decayed_fn)` (`ranking.rs:158-171`) is the exact pattern.

Add to `ranking.rs`:

```rust
pub(crate) struct DemoteSupersededRequest<'a> {
    pub(crate) items: &'a [AssembledContextItem],
}

/// Reorder so a superseded fact sits immediately after its successor, when
/// the successor is present in the same pack. Pure: no I/O, no clock.
pub(crate) fn demote_superseded(request: DemoteSupersededRequest<'_>) -> Vec<AssembledContextItem>;
```

Algorithm, fully determined by the signature and the three tests:

1. Collect the `fact_id`s present.
2. An item is demoted when its `reconciliation.relations` contains a relation with `outcome` `Supersession` or `Correction` whose `superseded_by_fact_id` is in that set.
3. Move each demoted item to sit immediately after its successor.

Apply the successor move once per item. The relation graph is acyclic by construction — a successor's validity interval follows its predecessor's — so one pass suffices. If a cycle is possible, bound the passes and stop; never write an unbounded loop.

**Synthetic fact ids.** Not every assembled item is backed by a fact row. `views.rs` produces `fact_id` values of the form `episode_fallback:<id>` (line 73), `facet:<label>` (262), `map:hub:<id>` (410) and `map:community:<id>` (432). None of these can appear in a `claim_relation`, so they will never match a predecessor or successor and are never demoted. That is the desired behaviour — do not "fix" it by parsing the prefix.

It does mean the demotion silently no-ops on facet-only or map-only packs. That is correct, not a bug: there is no successor to promote. Say so in the Task 4 commit body so a future reader does not read the no-op as a regression.

**`fact_id` is always non-empty** — `AssembledContextItem` derives `Default` but every construction site sets it explicitly (`experience.rs:168`, `views.rs:345`), and no production site emits an empty string. Do not add an empty-id guard; there is no case for it (KISS).

Call it from `retrieval.rs` at the single point Task 3 established. Nothing else changes there.

**Note what step 2 needs:** the successor's **fact id**. `ClaimRelationSummary` does not carry one — Task 1 removed `counterpart_fact_id`, and `reason_code` is not a pointer. Add `superseded_by_fact_id: Option<String>` to `ClaimRelationSummary`, populated in Task 3 only for `Supersession`/`Correction`. One field, one producer, one consumer; `claim_relation` still stores the truth and nothing recomputes it. Do not derive direction from string-matching `outcome`, and do not re-derive it from `claim_ids`.

- [x] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p memory_mcp --test assemble_context_supersession_ranking --features test-fixtures`
Expected: **1 passed** — `demotes_predecessor_below_successor_in_an_assembled_pack`.
Step 1 keeps the wiring level to exactly one test, so the three policy rules are
asserted only as unit tests in `ranking.rs`; an expectation of three here would
contradict that instruction.

- [x] **Step 5: Prove the demotion test detects the original defect**

ADR-0073 requires a regression scenario to demonstrate it fails against the defect. Temporarily comment out the reordering block, run the three tests, and confirm `demotes_predecessor_below_successor` fails while the other two pass. Restore the code and re-run. Record the output in the commit body.

- [x] **Step 5b: Run the full gate set**

```bash
cargo test -p memory_mcp --features fs-watch,mcp-apps,streamable-http && \
cargo fmt --all --check && \
cargo clippy --workspace --all-targets \
  --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings
```
Expected: all green.

- [x] **Step 7: Commit**

```bash
git add crates/memory-mcp/src/memory/retrieval crates/memory-mcp/tests/assemble_context_supersession_ranking.rs
git commit -m "feat(retrieval): rank a superseded fact below its successor when both are assembled"
```

---

### Task 5: A real correction scenario, replacing a misnamed test

`longmem_acceptance.rs:134` carries a test name claiming supersession while its body calls `invalidate` — which performs **retraction** (ADR-0009's deliberate opposite). It passes, and it passes for the wrong reason: it proves retraction works, under a name that promises supersession.

ADR-0074 requires this be replaced by a genuine correction scenario, because until it exists no test in the repository exercises the behavior Phase 0 just implemented.

**Files:**
- Modify: `crates/memory-mcp/tests/longmem_acceptance.rs` (replace the misnamed test)
- Test: same file.

**Interfaces:**
- Consumes: everything Tasks 1–4 produced.
- Produces: an acceptance test that distinguishes correction from retraction by observable outcome.

- [x] **Step 1: Read the existing test and name the defect**

```bash
sed -n '120,175p' crates/memory-mcp/tests/longmem_acceptance.rs
```
Expected: the body calls `invalidate`. If it does not, stop — this task's premise changed.

- [x] **Step 2: Write the replacement failing test**

Replace the misnamed test with:

```rust
#[tokio::test]
async fn corrected_fact_supersedes_the_stale_value_in_the_latest_view() {
    // Arrange: ingest episodes establishing "Atlas budget is $1M" then
    // "Atlas budget is $2M", and drive extraction so the $1M claim reconciles
    // as superseded by the $2M claim. `seed_fact_at` cannot do this — see below.
    let service = TestMemory::new(false);
    // …ingest + extract via common::ingest_episode and ExtractCapability,
    // following tests/claim_reconciliation_e2e.rs…
    //
    // Act
    let items = AssembleContextCapability::assemble_context_from_service(
        &service,
        AssembleContextRequest {
            query: "atlas budget".into(),
            as_of: None,
            budget: 5,
            fact_types: vec![],
            view_mode: None,
            window_start: None,
            window_end: None,
            access: None,
            compact: false,   // required: defaults to true, which omits reconciliation
        },
    ).await.expect("future context should assemble");
    //
    // Assert 1 — the stale value is not what the reader is served first.
    assert_ne!(items[0].content, "Atlas budget is $1M");
    //
    // Assert 2 — this is correction, not retraction: the stale value's record
    // still exists and remains reachable for provenance.
    assert!(items.iter().any(|i| i.content == "Atlas budget is $1M"),
        "the superseded value remains retrievable, not deleted");
    //
    // Assert 3 — the relation is visible on the stale item.
    assert!(items.iter().filter(|i| i.content == "Atlas budget is $1M")
        .all(|i| i.reconciliation.is_some()));
}
```

The `AssembleContextRequest` literal is exact, not a sketch — copy the field set from the test being replaced (`longmem_acceptance.rs:28-39`). `compact: false` is load-bearing: without it the field is omitted by Task 3 Step 6 and Assert 3 fails for a reason unrelated to correctness.

Assert 2 is the one that distinguishes correction from retraction. A test asserting only Assert 1 would also pass against `invalidate` — which is precisely the mistake the test being replaced makes.

**Arranging the relation is the hard part of this task, and `seed_fact_at` will not do it.** It inserts a fact row and creates no claim (`tests/common/mod.rs:135`), so two seeded facts have no relation between them and no reordering can occur. Drive the real pipeline — `common::ingest_episode` then `ExtractCapability`, as `tests/claim_reconciliation_e2e.rs` does — so the claim layer actually reconciles. Read that file first; it is the reference for producing relation rows.

If the test rollout stage does not emit relations, insert the `claim` and `claim_relation` rows directly and say so in the commit body. Do not weaken Assert 1 to make the test pass.

- [x] **Step 3: Run it to verify it fails**

Run: `cargo test -p memory_mcp --test longmem_acceptance corrected_fact_supersedes_the_stale_value_in_the_latest_view --features test-fixtures`
Expected: FAIL on Assert 1. If it passes immediately, your arrangement produced no relation — check that the correction use case actually ran.

- [x] **Step 4: Run it to verify it passes**

Expected after Tasks 1–4: **PASS**.

- [x] **Step 5: Delete the old test**

Remove the misnamed test entirely rather than leaving it beside the new one. Two tests covering overlapping ground, one named for behavior it does not exercise, is how the next reader gets misled.

- [x] **Step 6: Run the full acceptance suite**

```bash
cargo test -p memory_mcp --test longmem_acceptance --features test-fixtures
```
Expected: all pass.

- [x] **Step 7: Commit**

```bash
git add crates/memory-mcp/tests/longmem_acceptance.rs
git commit -m "test: replace misnamed supersession test with a real correction scenario"
```

---

### Task 6: Prove it end to end

Phase 0's real evidence is not "the tests pass." It is that the knowledge-update number in an external evaluation is no longer structurally incapable of moving. Before Phase 0, `assemble_context` never read a relation, so no reconciliation outcome could affect any reader-visible metric — the number was pinned by construction.

**Files:**
- Modify: none required unless a measurement gap is found
- Test: `crates/memory-mcp/tests/assemble_context_reconciliation.rs` (extend)

**Interfaces:**
- Consumes: all prior tasks.
- Produces: a recorded measurement, not a code change.

- [x] **Step 1: Add a test that the relation changes a response, not just a field**

The naive shape of this test — assemble once with the relation, assemble again without it, compare the leaders — **cannot work**, and failing to notice that is the easiest way to produce a green test proving nothing. Two assemblies of the same query return the *first* result from the context cache (Task 3 Step 5), so the second assertion would compare the cached list against itself.

Use **two separate services** instead:

```rust
mod common;

#[tokio::test]
async fn reconciliation_changes_which_fact_leads_the_pack() {
    // Arrange: two independent in-memory services (common::TestMemory::new),
    // each with facts A ("atlas budget is $1M") and B ("atlas budget is $2M").
    // In the first, run the pipeline so A's claim reconciles as superseded by B's.
    // In the second, do not reconcile — the claims stay independent.
    //
    // Act: assemble the identical query on each service. Separate services have
    // separate caches, so neither result is served from cache.
    //
    // Assert: the reconciled service leads with B; the unreconciled one does not
    // necessarily — assert only that the reconciled lead is B.
}
```

The point is not that the unreconciled service leads with A. It may still rank B first on its own merits. The contract under test is narrow and must be stated narrowly: **when a supersession exists, B leads.** A test that asserts "without reconciliation A leads" would be asserting something the ranker never promised.

Each service needs its own namespace and database — `common::TestMemory::new` allocates a fresh one per call (`tests/common/mod.rs:35-38`), so two calls give two isolated stores.

- [x] **Step 2: Run it and confirm it passes**

Expected: **PASS**. If it fails, Phase 0 did not achieve its goal — the relation is attached but not influential, and Tasks 3–4 need revisiting.

- [x] **Step 3: Run the eval harness**

The binary is `memory-eval` (`crates/eval-harness/Cargo.toml:13`, `path = "src/main.rs"`), not `eval-harness`, and there is no `compare` subcommand and no `--systems` flag. The real surface is `run` / `prepare-corpus` / `merge`, with `--profile`, `--artifact`, `--baseline`, and repeatable `--suite` (`crates/eval-harness/src/cli.rs:13-22`).

```bash
cargo run -p eval-harness --bin memory-eval -- run \
  --profile evals/profiles/release.json \
  --artifact target/eval/after-reconciliation.json \
  --suite longmemeval
```
Expected: it runs and prints a `profile=… total=… passed=…` summary line plus one `gate=…` line per gate.

Baseline arms (bm25 / dense / hybrid) are **profile** definitions, not a CLI flag. Phase 2 of the spec adds those profiles; this task runs the existing profile and reports the knowledge-update slice alongside the retrieval slice without merging them into one number.

**If the harness cannot run**, say so in the handoff rather than substituting a local assertion. `target/` is gitignored, so no committed baseline artifact exists to compare against, and a locally fabricated baseline would not be evidence.

- [x] **Step 4: Record the before/after knowledge-update number**

Report the number, the corpus, the reader, and the label-trust class. If the number did not move, Phase 0 is not complete — report that, do not explain it away.

- [x] **Step 4 result: recorded 2026-10-05. The number did not move, for a reason the plan did not foresee.**

Command: `cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/release.json --artifact target/eval/after-reconciliation.json --baseline evals/baselines/one-active-namespace-release.json`

Result: 117/117 cases, 9/9 gates, `RESULT: PASSED`, byte-identical to the baseline — `mrr=0.9918`, `recall_at_5=1.0000`, `top_1_hit_rate=0.9836`, `claim_f1=1.0000`.

Two blockers, one of them structural and neither of them the retrieval code:

1. **The harness ran at `shadow`.** `eval-harness` built its service without a claim rollout stage, so every run measured the *absence* of the reconciliation read path and would have published that as evidence about the feature. Fixed in this task: the harness now runs at `evidence`. This does not promote the deployment default — `CLAIM_RECONCILIATION.md`'s thresholds remain the operator's to verify.
2. **The number does not exist.** There is no knowledge-update metric in the harness: no key matching knowledge / update / temporal / supersession appears anywhere in the artifact. `CLAIM_RECONCILIATION.md` lists seven metrics and only `claim_precision`/`recall`/`f1` are implemented — `supersession_recall`, `temporal_ambiguity_rate` and `projection_precision` have neither code nor key. The retrieval suites also seed facts without the lineage that produces relations, so nothing there is positioned to move.

Consequence, stated rather than explained: Phase 0's production code is complete and mutation-verified in-process, but **its external evaluation evidence is not established**. The in-process proof — that a relation changes which fact leads a pack — is recorded in the Task 4 and Task 6 tests and their commit bodies. The external proof requires a metric that Phase 2 of the spec adds and this plan places out of scope.

- [x] **Step 5: Commit**

```bash
git add crates/memory-mcp/tests/assemble_context_reconciliation.rs
git commit -m "test: prove reconciliation influences which fact leads the assembled pack"
```

---

## Principles check

How this plan satisfies each principle, and the specific step that would violate it. A reviewer rejecting this plan should reject the named step, not the intent.

**SRP.** Each task owns one deliverable. Task 2 adds a query, Task 3 attaches data, Task 4 decides order, Task 5 replaces a misnamed test, Task 6 measures. `retrieval.rs` keeps only a call site; the projection lives in `knowledge/api.rs` (Task 3 Step 3) and the ordering policy in `ranking.rs` as a pure function (Task 4 Step 3). *Violation to reject:* putting either loop inline in `retrieval.rs`, which is already a 1409-line dispatcher.

**OCP.** Adding a relation outcome that demotes means adding a variant to the `outcome` match — no existing branch changes. *Violation to reject:* a new demotion rule keyed on `reason_code` strings, which closes the extension point permanently.

**LSP.** `RelationReadPort` has one method, so no implementor can depend on behaviour it does not provide. The existing `ClaimStore` doubles (`claims_policy/worker.rs:466`, `projection.rs:406`) are unaffected in meaning. *Violation to reject:* giving `RelationReadPort` a second method "just in case".

**ISP.** `memory` depends on `RelationReadPort`, not on the ten-method `ClaimStore`. A use case receives the narrow dependency it uses. *Violation to reject:* the earlier `Arc<dyn ClaimStore>` field in `AssembleContextDeps`, which would make assembly depend on every claim-store method including `commit_reconciliation_page`.

**DIP.** `memory/retrieval` depends on the interface declared in `knowledge/api.rs`; `SurrealClaimStore` stays behind it. Both are swappable, which is what makes the policy unit-testable without a database.

**DRY.** One query (`select_relations_by_fact`), one port, one policy function, one call site. The three policy rules are asserted once each, as unit tests — the wiring test deliberately does not repeat them (Task 4 Step 1). *Violation to reject:* re-deriving direction in the reader from `claim_ids`, or a second fingerprint alongside `context_fingerprint`.

**KISS.** The demotion rule has no tunable coefficient: demote only when the successor is present. Binary, one condition, no magic number to tune against the metric. *Violation to reject:* a weight constant, or a demote-by-group rule needing a new classification axis on the hot path.

**YAGNI.** No `Belief` type, no intent router, no typed retention, no consolidation worker, no migration, no cache-bypass parameter, no new MCP tool. `eval_support.rs` is untouched and `RelationForFact` stays `pub(crate)` — Task 2 Step 6 records the reader method that was cut and why. Each deferral carries a reason in the spec, not merely a skip. *Violation to reject:* adding a cache flag "to make the test easier" (a fixture problem, not a solution), or re-adding the `ClaimEvidenceReader` method "because the harness might want it later".

**DDD / bounded contexts.** Claims and relations belong to `knowledge` (ADR-0058); the port is declared there and `memory` consumes it. `memory/retrieval` never composes claim SQL. Policy lives in the owning context as pure functions (ADR-0066). Supersession and retraction stay distinct operations (ADR-0009).

**12-factor.** Config comes from the environment and is declared in one place; this plan **adds no configuration at all**. The demotion rule is a compile-time decision with no env var, because a rule whose activation is tunable at deploy time is a rule nobody has decided. It introduces no new dependency — only `tokio`, `async-trait`, `serde`, already present. Storage stays behind `DbClient`/`SurrealClaimStore`, so embedded and remote remain a URL, not a code path. The existing `CONTEXT_CACHE_SIZE = 512` hardcode is *reported* in Task 3 Step 5 as a constraint to work around, not *extended* by this plan.

---

## Out of Scope

Deliberately not in this plan. Each was rejected with a reason in the spec.

- Writing `valid_to`. Never written, and writing it here would trip `latest_registered_migration_is_expected` (`knowledge/claims.rs:792`). It is a Phase 1 precision fix, not a Phase 0 prerequisite.
- `pair_fingerprint` population. Phase 1. Do not add a second fingerprint; either populate it deterministically or drop it for `context_fingerprint`.
- Any migration. `valid_to` already exists; no new table is required here.
- Intent-routed retrieval, typed retention, trust propagation. Spec Phases 3, 4 and 6.
- A `Belief` type, `disputed`/`uncertain` statuses, or a new confidence field. Duplicates `ClaimRelationOutcome` and revives a deliberately retired state.
- A consolidation worker. Contradicted by LongMemEval §5.2 and by this codebase's evidence-preserving design.
- Projecting relations into `explain`. `explanation.rs` reads only facts and entities today. ADR-0074 records this as a deliberate open choice, not a precondition. Decide it explicitly during review; do not let it happen by accident.