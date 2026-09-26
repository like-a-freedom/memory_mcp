# Release acceptance evidence

Revision: `4ea7c39` (branch `ddd-refactorings`)
Features where two are given: `fs-watch,mcp-apps,streamable-http`
Full-feature run adds `test-fixtures`.

Every command below was executed on this revision. Results are
reported as observed, including the one gate that initially
failed and what fixed it.

## Local gate set

| Command | Result |
|---|---|
| `cargo fmt --all --check` | pass |
| `cargo clippy --workspace --all-targets --features fs-watch,mcp-apps,streamable-http --locked -- -D warnings` | pass, no warnings |
| `cargo clippy -p ui --target wasm32-unknown-unknown --all-targets --locked -- -D warnings` | pass |
| `cargo test --workspace --lib --bins --tests --locked` | pass |
| `cargo test -p memory_mcp --lib --bins --tests --features fs-watch,mcp-apps,streamable-http,test-fixtures --locked` | pass |
| `cargo check -p memory_mcp --all-targets --no-default-features --features streamable-http,test-fixtures --locked` | pass |
| `cargo check -p memory_mcp --all-targets --no-default-features --features control-plane,test-fixtures --locked` | pass |
| `cargo check -p memory_mcp --all-targets --no-default-features --locked` | pass |
| `python3 -m unittest discover -s scripts/ci -p 'test_*.py'` | pass, 50 tests |

## Supported profile combinations

| Features | Result |
|---|---|
| `fs-watch` | pass |
| `fs-watch,mcp-apps` | **failed, then fixed** (see below) |
| `streamable-http` | pass |
| `streamable-http,mcp-apps` | pass |

The `fs-watch,mcp-apps` profile failed on first run:
`tests/provisioning_app_sessions.rs` was gated on `mcp-apps`,
but the module it exercises (`provisioning`) is composed only
under `streamable-http`. The default and full-feature suites
both enable `streamable-http`, so the defect was invisible
until the profile matrix was compiled explicitly. Fixed in
`4ea7c39` by gating the suite on `streamable-http`.

## Evaluation jobs

Run as the actual harness, not as harness unit tests.

| Profile | Cases | Passed | Quality failed | Invalid | Gates | Verdict |
|---|---|---|---|---|---|---|
| `pr` (vs `evals/baselines/one-active-namespace-pr.json`) | 113 | 113 | 0 | 0 | 7/7 | PASSED |
| `release` (vs `evals/baselines/one-active-namespace-release.json`) | 117 | 117 | 0 | 0 | 9/9 | PASSED |
| `response_size` | 61 | 61 | 0 | 0 | — | PASSED |

### Quality gates

Recorded as observed in the run artifacts
(`target/evals/{pr,release}.json`), and compared against
`evals/baselines/one-active-namespace-{pr,release}.json`.

| Suite | Metric | Observed | Baseline in run | Status |
|---|---|---|---|---|
| local-retrieval | recall_at_5 | 1.0000 | 1.0000 | passed |
| local-retrieval | mrr | 0.9918 | 0.9918 | passed |
| local-retrieval | top_1_hit_rate | 0.9836 | 0.9836 | passed |
| extraction | entity_f1 | 0.7500 | 0.7500 | passed |
| claim-reconciliation | claim_precision | 1.0000 | 1.0000 | passed |
| claim-reconciliation | claim_recall | 1.0000 | 1.0000 | passed |
| lifecycle | action_grounding_pass_rate | 1.0000 | 1.0000 | passed |
| lifecycle | poisoning_pass_rate | 1.0000 | 1.0000 | passed |
| external-retrieval | answer_presence_proxy_at_5 | 1.0000 | none recorded | passed |

Every **gated** metric is identical to its recorded baseline.
Retrieval, extraction, claim reconciliation and the lifecycle
action/poisoning checks are unchanged by the refactor,
including the bi-temporal visibility, tier ranking and
claim-projection paths that were rewired.

**One difference worth recording rather than glossing.** The
`external-retrieval` suite (2 cases) reports a different metric
set from the baseline file: this run reports
`answer_presence_proxy_at_5`, while the baseline records
`mrr`, `recall_at_5` and `top_1_hit_rate` for that suite. The
run's own gate for it carries no baseline, so it was not
regression-checked, and the harness reported it as passed on its
hard floor of 0.9000. This is a pre-existing difference in how
that suite is measured, not a result of this work, and it is
left as-is: the external corpora are the ones that need
`prepare-eval-corpora` and a separate run, so nothing here
certifies external-retrieval quality. Treat that suite as
unverified in this evidence set.

Response-size savings, unchanged by this work:
assemble overall 38.77% smaller responses, explain 14.15%.

## Recorded limitations

- **The migration is not complete.** An adversarial review of
  `791f903..5a41c16` found that this branch delivers the bounded
  contexts' *public interfaces*, their consumer-owned ports, the
  owner-scoped accessors, the composed bootstrap integration
  adapters, and the renamed surface — but **not** the movement of
  the 330 remaining legacy files out of `service/`, `control/`,
  `http/registry/` and `storage/`. In particular:
  - `src/shared/` and `src/platform/`, which the plan calls for,
    do not exist.
  - `ServiceContext` still exists. It no longer reaches the MCP
    tool handlers (they now take `tools::context::ToolContext`),
    but it remains the shared container the capabilities adapt.
  - The manifest's `legacy: … -> Phase N` column records *intended
    destination and intended removal phase*, not completed
    movement. Most of those files have not moved yet.
  - The plan's own checkboxes are deliberately left unchecked for
    the items that are not done; see the plan for the true status.
  Phases 0–2 (surface changes, `ui` rename, flag retirement,
  manifest, docs, release evidence) are complete. Phase 5's
  "Replace `ServiceContext` with explicit consumer-owned
  ports/dependencies" is complete at the transport boundary and
  inside the capabilities, but the legacy module split it implies
  is outstanding.
- `ner_progress_channels::blocked_gliner_refresh_does_not_delay_initialize`
  hangs intermittently when test targets run concurrently. It
  passes in isolation (2.02s) and passes in the full suite when
  run serially. Pre-existing: the test performs a blocking
  `join()` on a thread gated by a TCP fixture, with no timeout.
  It has not been changed here, as it is outside this plan's
  scope. It is a real hazard for CI and should get a timeout.
- Tests requiring local model fixtures are `ignored` and did
  not run: the GLiNER checkpoint
  (`urchade--gliner_multi-v2.1`), the multilingual-e5-small
  embedding model, and the 1.6 GB VAGO LFM2.5 checkpoint. These
  are missing evidence, not passes.
- `ContextStoreClient::select_episodes_via_entity` is unchanged
  and still joins `episode`+`fact`+`edge`. It was deliberately
  not split: the target is a composition-layer read, but that
  needs a benchmark of query decomposition first. Recorded in
  `docs/architecture/decisions/0001-typed-record-accessors.md`
  and in the migration manifest.

## Behaviour change requiring a release note

`invalidate` now refuses a record id that is not `fact:<id>`.
Previously the tool's existence check and its close both
derived their table from the caller's string, so `edge:` and
`triple:` ids were closed successfully and other ids produced a
schema violation. The documented contract is unchanged ("invalidate
a fact"); callers relying on the old, undocumented behaviour will
now receive a validation error.
