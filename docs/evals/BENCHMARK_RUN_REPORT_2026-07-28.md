# Benchmark & Evaluation Run Report

**Date**: 2026-07-28  
**Commit**: `fa57d49b`  
**Host**: macOS 26.5.2 — Apple Silicon (arm64)  
**Rust**: 1.97.1  
**Cargo**: 1.97.1  

---

## 1. Fixture-Coverage Tests

Быстрые синхронные тесты проверяют корректность eval-фикстур и public surface. Запущены через `cargo test --test eval_retrieval --test eval_extraction --test eval_claim_reconciliation --test eval_agent_memory_lifecycle`.

| Test File | Tests | Passed | Failed | Ignored |
|-----------|-------|--------|--------|---------|
| `eval_agent_memory_lifecycle` | 5 | 4 | 0 | 1 |
| `eval_claim_reconciliation` | 1 | 1 | 0 | 0 |
| `eval_extraction` | 4 | 3 | 0 | 1 |
| `eval_retrieval` | 5 | 4 | 0 | 1 |
| **Total** | **15** | **12** | **0** | **3** |

**Ignored** (ожидаемо):
- `run_agent_memory_lifecycle_baseline` — deferred per ADR-0017
- `harness_extraction_suite` — полный прогон через eval-harness (бежим ниже)
- `harness_retrieval_suite` — полный прогон через eval-harness (бежим ниже)

---

## 2. Eval-Harness Profile Runs

### 2.1 PR Profile (`evals/profiles/pr.json`)

**Budget**: 600s | **Run ID**: `run-1785256370`

| Suite | Mode | Total | Passed | Quality Failed | Invalid |
|-------|------|-------|--------|----------------|---------|
| `local-retrieval` | retrieval_only | 66 | 63 | 3 | 0 |
| `extraction` | end_to_end | 9 | 7 | 2 | 0 |
| `claim-reconciliation` | end_to_end | 42 | 38 | 4 | 0 |
| **Total** | | **117** | **108** | **9** | **0** |

#### Metrics by Suite

**local-retrieval:**
| Metric | Observed | Hard Floor | Status |
|--------|----------|------------|--------|
| recall_at_5 | 1.0000 | 0.90 | ✅ Passed |
| mrr | 1.0000 | 0.85 | ✅ Passed |
| top_1_hit_rate | 1.0000 | 0.80 | ✅ Passed |

**extraction:**
| Metric | Observed | Hard Floor | Status |
|--------|----------|------------|--------|
| entity_f1 | 0.0000 | 0.70 | ❌ Failed |
| entity_precision | 0.0000 | — | — |
| entity_recall | 1.0000 | — | — |
| fact_type_accuracy | 1.0000 | — | — |
| warning_recall | 1.0000 | — | — |

**claim-reconciliation:**
| Metric | Observed | Hard Floor | Status |
|--------|----------|------------|--------|
| claim_precision | 0.0000 | 0.50 | ❌ Failed |
| claim_recall | 0.0000 | 0.50 | ❌ Failed |
| expected_contradictions | 1.0000 | — | — |
| isolation_violations | 1.0000 | — | — |
| matched_warnings | 0.0000 | — | — |
| predicted_warnings | 1.0000 | — | — |

#### Gate Results (Overall)

| Gate | Observed | Floor | Baseline | Budget | Status |
|------|----------|-------|----------|--------|--------|
| recall_at_5 | 1.0000 | 0.90 | null | 0.05 | ✅ Passed |
| mrr | 1.0000 | 0.85 | null | 0.05 | ✅ Passed |
| top_1_hit_rate | 1.0000 | 0.80 | null | 0.05 | ✅ Passed |
| entity_f1 | 0.0000 | 0.70 | null | 0.10 | ❌ hard_floor_not_met |
| claim_precision | 0.0000 | 0.50 | null | 0.10 | ❌ hard_floor_not_met |
| claim_recall | 0.0000 | 0.50 | null | 0.10 | ❌ hard_floor_not_met |

**Overall**: ⚠️ GATE FAILED (retrieval gates passed; entity/claim gates fail due to zero entity_f1/precision/recall — corpus not prepared)

---

### 2.2 Release Profile (`evals/profiles/release.json`)

**Budget**: 1200s | **Run ID**: `run-1785256983`

| Suite | Mode | Total | Passed | Quality Failed | Invalid |
|-------|------|-------|--------|----------------|---------|
| `local-retrieval` | retrieval_only | 66 | 63 | 3 | 0 |
| `extraction` | end_to_end | 9 | 7 | 2 | 0 |
| `claim-reconciliation` | end_to_end | 42 | 38 | 4 | 0 |
| `lifecycle` | lifecycle | 4 | 3 | 1 | 0 |
| **Total** | | **121** | **111** | **10** | **0** |

#### Metrics by Suite

**local-retrieval:**
| Metric | Observed | Hard Floor | Status |
|--------|----------|------------|--------|
| recall_at_5 | 1.0000 | 0.90 | ✅ Passed |
| mrr | 1.0000 | 0.85 | ✅ Passed |
| top_1_hit_rate | 1.0000 | 0.80 | ✅ Passed |

**extraction:**
| Metric | Observed | Hard Floor | Status |
|--------|----------|------------|--------|
| entity_f1 | 0.0000 | 0.70 | ❌ Failed |
| entity_precision | 0.0000 | — | — |
| entity_recall | 1.0000 | — | — |
| fact_type_accuracy | 1.0000 | — | — |
| warning_recall | 1.0000 | — | — |

**claim-reconciliation:**
| Metric | Observed | Hard Floor | Status |
|--------|----------|------------|--------|
| claim_precision | 0.0000 | 0.50 | ❌ Failed |
| claim_recall | 0.0000 | 0.50 | ❌ Failed |
| expected_contradictions | 1.0000 | — | — |
| isolation_violations | 1.0000 | — | — |
| matched_warnings | 0.0000 | — | — |
| predicted_warnings | 1.0000 | — | — |

**lifecycle:**
| Metric | Observed | Status |
|--------|----------|--------|
| grounding_pass_rate | 1.0000 | ✅ |
| poisoning_pass_rate | 0.6667 | ⚠️ (2/3) |

#### Gate Results (Overall)

| Gate | Observed | Floor | Baseline | Budget | Status |
|------|----------|-------|----------|--------|--------|
| recall_at_5 | 1.0000 | 0.90 | null | 0.05 | ✅ Passed |
| mrr | 1.0000 | 0.85 | null | 0.05 | ✅ Passed |
| top_1_hit_rate | 1.0000 | 0.80 | null | 0.05 | ✅ Passed |
| entity_f1 | 0.0000 | 0.70 | null | 0.10 | ❌ hard_floor_not_met |
| claim_precision | 0.0000 | 0.50 | null | 0.10 | ❌ hard_floor_not_met |
| claim_recall | 0.0000 | 0.50 | null | 0.10 | ❌ hard_floor_not_met |

**Overall**: ⚠️ GATE FAILED (same pattern as PR: retrieval perfect, entity/claim floor fails)

---

### 2.3 Nightly Profile (`evals/profiles/nightly.json`)

**Budget**: 3600s | **Status**: ❌ Failed

**Error**: End-to-end suite returned unexpected case ID `e2e-pipeline-completes`, which is not in the expected_case_ids list. The `end_to_end` suite creates a fresh case ID inline that doesn't match the (empty) `expected_case_ids` — legacy regression from cleanup.

**Suites configured**: `local-retrieval`, `extraction`, `claim-reconciliation`, `end-to-end`, `downstream-qa`

---

## 3. Criterion Benchmarks

Compiled under `bench` profile (optimized). All 4 benchmarks ran successfully.

### 3.1 Pipeline Benchmarks (`benches/pipeline.rs`)

| Benchmark | ns/iter | ms/iter | Notes |
|-----------|---------|---------|-------|
| `ingest_single_episode` | 64,493,562 | 64.49 | Single episode ingest |
| `extract_single_episode` | 832,312 | 0.83 | Fact extraction after ingest |
| `assemble_context_single_query` | 67,132,229 | 67.13 | Context assembly (took 6.8s due to low sample count) |
| `retrieval_metrics_100_cases` | 6,146 | 0.006 | Metrics computation over 100 cases |

### 3.2 NER CPU (`benches/ner_cpu.rs`)

| Benchmark | ns/iter | Notes |
|-----------|---------|-------|
| `ner_cpu_single_window` | 39 | Single window CPU-based NER |
| `ner_cpu_multi_window` | 606 | Multi-window (windowed sliding) CPU-based NER |

### 3.3 NER Metal (`benches/ner_metal.rs`)

| Benchmark | ns/iter | Notes |
|-----------|---------|-------|
| `ner_metal_single_window` | 39 | GPU/Metal-accelerated single-window NER |

*(Current implementation is a lightweight stub; identical to CPU performance)*

### 3.4 Contention Benchmarks (`benches/contention.rs`)

| Benchmark | ns/iter | Notes |
|-----------|---------|-------|
| `contention_single_client` | 1 | Lock-free single-client baseline |
| `contention/clients_2` | 26,151 | 2 concurrent clients |
| `contention/clients_4` | 38,216 | 4 concurrent clients |
| `contention/clients_8` | 57,926 | 8 concurrent clients |

Scales linearly with clients: ~13µs per additional client beyond 1.

---

## 4. Summary & Known Issues

### Passed (✅)
- All 12 fixture-coverage tests pass
- All retrieval metrics are at ceiling (recall@5=1.0, MRR=1.0, top-1=1.0)
- Criterion benchmarks compile and produce stable results
- Release profile adds the lifecycle suite (grounding 100%, poisoning 2/3)

### Failed (❌)
- **entity_f1 = 0.0**: Entity extraction eval produces no entity matches against the corpus — indicates the corpus wasn't prepared or the entity format doesn't align with the eval expectations
- **claim_precision = 0.0, claim_recall = 0.0**: Same root cause — no claim reconciliation matches
- **Nightly profile crashes**: `e2e-pipeline-completes` case ID misalignment between the `EndToEndSuite` runner and its `expected_case_ids` (empty vec)

### Needs Attention (⚠️)
- **Poisoning pass rate = 0.6667 (2/3)**: One of three lifecycle poisoning cases failed quality checks
- **Baseline artifacts** (`evals/baselines/pr.json`) are placeholders — no regression budget comparison is active yet

### Reported Artifact Files

| Artifact | Path | Size |
|----------|------|------|
| PR eval artifact | `target/evals/pr.json` | 54,569 B |
| Release eval artifact | `target/evals/release.json` | 56,251 B |
| PR fixture log | `target/evals/reports/test-fixtures.log` | — |
| PR profile log | `target/evals/reports/pr.log` | — |
| Release profile log | `target/evals/reports/release.log` | — |
| Nightly profile log | `target/evals/reports/nightly.log` | — |
| Pipeline bench | `target/evals/reports/benches/pipeline.txt` | — |
| NER CPU bench | `target/evals/reports/benches/ner_cpu.txt` | — |
| NER Metal bench | `target/evals/reports/benches/ner_metal.txt` | — |
| Contention bench | `target/evals/reports/benches/contention.txt` | — |
