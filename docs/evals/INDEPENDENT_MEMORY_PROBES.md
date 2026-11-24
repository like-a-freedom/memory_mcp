# Independent memory probes and deterministic acceptance

Status: standalone stdio/HTTP capture, generic paired integration, portable verification and deterministic candidate acceptance are implemented. Measured product results are separate in [the evidence report](DETERMINISTIC_MEMORY_QUALITY.md). Agent outcome confirmation remains optional and open.

## Contract

`ProbeInput` (`memory-mcp-independent-probe/v1`) fixes the query, principal label, `valid_at`, nonnegative `known_revision`, context budget, history SHA-256 and claim rollout stage. The budget is the number of returned context items, matching public `assemble_context.budget`; it is not a token count. The standalone captures support stdio and Streamable HTTP with the isolated `fixture-reader` principal label only (local store or newly bootstrapped local fixture tenant). It rejects other principal labels; it does not establish multi-tenant authorization coverage.

`ProbeOracle` is evaluator-only. `sufficient_evidence_alternatives` is OR across alternatives, AND across groups inside each alternative, and OR across event IDs inside a group. Every required group in one alternative must be represented. A forbidden event defeats otherwise sufficient evidence. An impossible required group is invalid evaluation. An empty alternatives array explicitly requires an empty response, including zero returned items; it does not prove that an agent abstained correctly. `required_claim_annotations` requires every listed annotation. `required_claim_annotation_alternatives` accepts any one complete listed set (AND within a set); it cannot be combined with the conjunctive field. Each annotation must bind to a source event in the sufficient-evidence oracle. This supports an oracle that accepts either of two equivalent top-ranked claims under a one-item budget without weakening the required status/relation check.

`ProbeObservation` contains the input/product fingerprints, bound source event IDs, returned item count (`budget_used`) and completion flag. Capture v2 also records optional, content-free claim annotations bound to returned sources: `evidence_status` and compact reconciliation `{outcome, role}` pairs. A `ProbeOracle` may require one or more exact annotation values or relation pairs for a source that is already part of its sufficient-evidence alternatives. All required annotations must be present; additional relations are permitted. This lets a fixed probe distinguish a returned fact from a returned fact whose disagreement status is made explicit. Oracles without annotation requirements preserve the original source-coverage metric. Missing observations and identity mismatches are errors, not successes. Incomplete, over-budget, prohibited, or annotation-incomplete evidence scores false. External observations are trusted normalized inputs: hashes detect identity mismatch, not dishonest capture. Independent execution requires a reviewed adapter and its raw capture receipts.

`ProbeScore` (`memory-mcp-independent-probe-score/v1`) records input, oracle, observation and product fingerprints plus `evidence_success`. It measures the registered sufficient-source coverage and any explicitly required typed annotations. It does not score claim text, answer correctness or agent usability. Existing memory-decision evaluations cover richer decision behavior. Existing paired v2 `direct_memory_success` retains its historical agent-observed meaning; this command does not replace it. Capture v1 remains readable with its historical source-only interpretation; capture v2 derives annotations from the public response. A v1 observation without annotations still scores under the original source-only oracle.

Fingerprints use SHA-256 of compact `serde_json` serialization of the typed value. Input/history fields follow Rust declaration order; timestamps are normalized by chrono. These are not raw pretty-printed file hashes or a general JSON canonicalization standard. Use `ProbeInput::sha256()` for input identity. History serializes a `Vec<SeedEpisode>` with fields `event_id`, `source_id`, `source_type`, `content`, `valid_at` in that order. The product fingerprint is SHA-256 of executable bytes.

## Run without an agent

```sh
cargo build --locked -p memory_mcp --bin memory_mcp

cargo run --locked -p eval-harness --bin memory-eval -- capture-independent-probe --transport stdio \
  --input /absolute/probe-input.json --history /absolute/history.json \
  --server-binary /absolute/memory_mcp --product-sha256 PRODUCT_SHA256 \
  --store /absolute/new-probe-store --claim-rollout-stage shadow \
  --output /absolute/new-capture.json
```

The store path must not exist; the parent must exist. Capture forms the fixed history through public `ingest`/`extract`, closes that session and starts a new session for exactly one fixed `assemble_context` read. It checks the product/history hashes, binds every returned source through observed ingest IDs, and verifies both snapshot coordinates. It records observed episode-to-event bindings, raw request/response, tool schema hash, rollout stage, transport and a curated non-secret `launch_profile` taken from the actual child command environment. Extractor/embedding metadata is derived from that profile rather than hand-written labels. This is selected launch-setting evidence, not complete runtime introspection. Credentials and physical store paths are excluded. For HTTP use `--transport streamable-http` and a `memory_mcp_http` binary built with `--features streamable-http,test-fixtures`. HTTP uses a new isolated tenant/server for formation and read; stdio closes and reopens its own isolated store. No agent calls, answers or oracle enter capture. The local fixture uses regex extraction and disabled embeddings; it is not a semantic-model benchmark.

Extract `capture.observation` into a standalone JSON file, or provide an equivalent observation from another reviewed runner, then score offline:

```sh
cargo run --locked -p eval-harness --bin memory-eval -- score-independent-probe \
  --input /absolute/probe-input.json --oracle /absolute/private-oracle.json \
  --observation /absolute/observation.json --product-sha256 PRODUCT_SHA256 \
  --output /absolute/new-score.json
```

These Cargo commands work in PowerShell as well as POSIX shells. On Windows,
pass the built product path ending in `target\debug\memory_mcp.exe`; Cargo
selects `memory-eval.exe` automatically. Offline scoring needs no product
executable, provider, model account or network.

Both commands reject existing output files. Scoring exits 0 when evaluation completes, including `evidence_success: false`; exit 2 means invalid input or execution failure. This is diagnostic scoring, not a promotion command. Integration tests run both CLIs with an empty environment, without a model account or vendor runner.

## Recorded smoke

[2026-11-21 stdio smoke](../../evals/independent-probes/2026-11-21-stdio-smoke/) records a synthetic two-event history against the actual locally built dirty-master binary. This checks transport and capture plumbing only. It was not a preregistered acceptance cohort and has no memory-quality or improvement claim. Raw responses contain generated IDs and transaction timestamps; repeatability compares the normalized observations, not raw byte identity.

## Historical standalone checkpoint

At the standalone checkpoint integration and acceptance were still open. The integrated contract and acceptance below supersede that status. The smoke alone never supported a product or agent-benefit claim.

[2026-11-21 transport smoke](../../evals/independent-probes/2026-11-21-transport-smoke/) repeats the same observed synthetic control twice per transport, with explicit regex extraction and disabled embeddings. Both repeats and both transports retrieve the expected pre-correction source at revision 3; all four source-coverage scores pass. This is a transport/scorer control, not a fresh acceptance cohort or evidence of improvement. Earlier stdio-only prototype captures lack the later launch-profile metadata and must not be used as configuration-attested acceptance artifacts.

Legacy HTTP paired metadata needs separate correction: its declared profile was not implemented by the legacy launch adapter. The [candidate report](TEMPORAL_CANDIDATE_AGENT_EVAL.md) preserves that limitation; new independent capture does not retroactively validate past paired runs.

[Launch-profile smoke](../../evals/independent-probes/2026-11-21-launch-profile-smoke/) is the later repeated control with non-secret launch settings captured from child commands and preserved episode/event bindings. Both repeats agree within each transport; all four source-coverage controls pass. Storage profiles differ deliberately between local stdio and multi-tenant HTTP. These captures support the plumbing/control claims only. They do not provide paired product acceptance or a fresh quality estimate.

## Typed conflict annotation control (2026-11-24)

[The typed-evidence probe](../../evals/paired/typed-evidence-probe-2026-11-24/) uses one newly written synthetic two-source history and a one-item context budget. Its frozen oracle requires the correction source plus `evidence_status=conflicting` and the compact `contradiction/conflict` relation. On the same locally built product binary and separate fresh stores, the evidence-stage public capture scored `evidence_success: true`; the shadow-stage control scored `false` because it returned no required typed annotation. The capture and offline score artifacts, product/evaluator hashes, and exact commands are recorded with the probe.

This closes a measurement gap: earlier independent probes could pass on source IDs even when an agent received no explicit conflict marker. It confirms this registered deterministic behavior at budget one; it is not an agent task outcome, powered comparison, generalization estimate, or memory treatment effect. No agent, model provider, Codex service, or API key was used.

## Integrated paired artifacts (2026-11-21)

`paired_agent::independent::run_with_independent_probes` composes any `PairedTrialBackend` with an independent `IndependentProbeBackend`. The latter receives only the fixed input and seed history, never the task oracle, agent queries or agent store. `PublicProbeBackend` forms a separate fresh store for every arm/trial and supports stdio and Streamable HTTP. `run-paired-codex --independent-probes <json>` is an optional adapter to this generic API; deterministic memory acceptance below requires no agent executable or account.

Register `Vec<FixedTrialProbe>` with exact case/trial keys, history/input, expected non-secret launch profile and evaluator-only evidence oracle. For a rollout-stage comparison, pin both stages in the product identities. The probe input and launch profile name the incumbent stage; the paired runner derives the candidate stage from its product identity for candidate capture. The input fingerprints therefore differ only in the preregistered stage field, while query, principal, snapshot, budget and history remain fixed. Comparison permits that one registered stage difference and rejects any other launch-profile difference. A missing probe, changed profile, unbound raw source, changed snapshot or detached observation is invalid. Capture receipts retain the actual per-arm input/product identities, raw response, source bindings, product and public-schema fingerprints. Offline `IndependentPairedArtifact::validate` re-derives observations and scores against the frozen per-arm inputs/oracles without invoking an agent.

The envelope schema is `memory-mcp-independent-paired/v1`; `agent_observed` nests an unchanged v2 artifact. Its legacy `direct_memory_success` remains **agent-observed source coverage**. `independent_memory[].score.evidence_success` is the separate fixed-probe metric. `compare_independent_runs` requires matching envelope/manifest identities, opposite arms, one run ID and identical registered launch settings. Old v2 artifacts cannot substitute for the envelope and remain readable through the old API. Source coverage alone is not decision correctness or evidence of improved agent usability. Legacy HTTP agent profiles remain configuration-unattested diagnostics; independent capture profile validation does not retroactively attest them.

## Deterministic decision acceptance

`accept-memory-candidate --corpus-root <registered-corpus-directory> --plan <frozen.json> --incumbent-first <index.json> --incumbent-second <index.json> --candidate-first <index.json> --candidate-second <index.json> --output <new.json>` consumes the richer `memory-decisions` replay channel `evidence_enabled_temporal_*`. It checks source assertions, source roles, disposition and relation outcomes through that channel's existing private oracle scorer. No fallback to agent-selected reads or the legacy/default channel is permitted.

Protocol `memory-mcp-deterministic-acceptance/v1` requires matching corpus/config/public schema, exact frozen case/family coverage, semantically equal repeats with distinct raw receipt fingerprints, positive frozen family-weighted gain, no evidence regressions, no ordinary-workflow losses, zero prohibited/unbound/unsupported-time/budget/nonrepeatability violations, and a fixed item ceiling. Missing data is invalid. Exit 0 accepts, 1 is a valid rejection, 2 is invalid input/execution. The report contains per-case wins/losses, denominators and a descriptive family bootstrap interval (seed 20261121, 10000 draws, 95%); acceptance uses the exact finite-cohort gain, not a population inference. Raw receipt diversity detects accidental reuse, not malicious fabrication.

Frozen development v4 and confirmation v5 manifests and the saved pre-fix binary identity live under `evals/paired/deterministic-quality-2026-11-21/`. Both use 8 disjoint subject histories, two ordinary controls, a 25 percentage-point family gain and a 5-item ceiling. They share synthetic scalar templates; this limits the claim to mechanism transfer. They do not prove model-mediated agent benefit, latency noninferiority, arbitrary languages or every selector. Confirmation outcomes must remain uninspected until mechanisms and their candidate binary are frozen.

### Fail-closed artifact verification

The acceptance CLI requires the actual registered corpus, not just its hash in a replay index. `load_registered_replay` verifies the corpus/configuration, exact case coverage, event payload/order, private oracle, public seed identities, extraction requests, monotone public revision checkpoints and the actual query, valid time, revision and budget. Raw sibling receipts are hashed, normalized and scored again. Changing a query or oracle and recomputing its receipt hash cannot redefine the registered task. Legacy index-only comparison is diagnostic and cannot enter this gate.

Every corresponding case in the second capture must have a distinct raw receipt fingerprint; changing only one case cannot hide reuse of the others. Supported/correction dispositions with wrong assertions are hard unsupported positive decisions. Promoting evidence to `current` against the oracle's role is a hard unsafe promotion even when aggregate gains are large. Conservative incomplete/unknown decisions remain visible failures and cannot be converted into success by omission.

Independent probe capture requires complete one-to-one bindings for the registered history, including empty-response probes. JSON-RPC errors and MCP `isError` responses are invalid capture, not successful abstention. The raw JSON and hashes establish reproducible provenance and detect accidental reuse; they are not cryptographic attestation of an untrusted runner.

### Compatibility matrix

| Artifact | Reader and meaning | Candidate acceptance |
|---|---|---|
| Legacy paired v2 | Existing paired API; agent-observed evidence coverage | Cannot replace independent probes |
| Independent-paired v1 | New envelope; unchanged legacy v2 nested alongside separate fixed-probe source coverage | Source coverage alone cannot qualify decision quality |
| Independent-comparison v1 | Validated paired envelope comparison | No agent-outcome or decision-gain inference |
| Memory-decisions replay v1, receipt v3 | Existing index comparison remains readable; registered loader rederives rich observation from raw receipts | Requires registered corpus, raw receipts and full repeated coverage |
| Deterministic acceptance plan/report v1 | Frozen finite-cohort gain, safety and resource rules | Exit 0 accepted, 1 valid rejection, 2 invalid execution/input |

An older capture missing required launch provenance is historical diagnostic input. A missing observation, changed public tool schema, missing registered history event or reused per-case raw capture cannot be silently upgraded to the stronger contract. Schema compatibility is a protocol property, not a product-quality score.
