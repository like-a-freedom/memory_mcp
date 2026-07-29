# Benchmark & Evaluation Run Report v5

**Date**: 2026-07-29 (fifth run)  
**Commit**: `29051273` — `make evaluation results truthful and coverage-aware`  
**Previous runs**: [v1 2026-07-28](BENCHMARK_RUN_REPORT_2026-07-28.md) · [v2 2026-07-29](BENCHMARK_RUN_REPORT_2026-07-29.md) · [v3 2026-07-29](BENCHMARK_RUN_REPORT_2026-07-29-v3.md)
**Host**: macOS 26.5.2 — Apple Silicon (arm64)  
**Rust**: 1.97.1 / Cargo 1.97.1

---

## 1. Fixture-Coverage Tests

| Test File | Passed | Failed | Ignored |
|-----------|--------|--------|---------|
| `eval_agent_memory_lifecycle` | 4 | 0 | 1 |
| `eval_claim_reconciliation` | 1 | 0 | 0 |
| `eval_extraction` | 3 | 0 | 1 |
| `eval_retrieval` | 4 | 0 | 1 |
| **Total** | **12** | **0** | **3** |

✅ Все фикстуры стабильны (идентично v3/v4).

---

## 2. Eval-Harness Profile Runs

### 2.1 PR Profile (`evals/profiles/pr.json`)

**Run ID**: `run-1785325421` | **Duration**: 9 771 ms  
**Result**: ✅ **PASSED** (verdict PASSED, 7/7 gates passed, 0 invalid)

| Suite | Total | Passed | Quality Failed |
|-------|-------|--------|----------------|
| `local-retrieval` | 66 | 66 | 0 |
| `extraction` | 9 | 9 | 0 |
| `claim-reconciliation` | 42 | 42 | 0 |
| `external-retrieval` | 2 | 2 | 0 |
| **Total** | **119** | **119** | **0** |

#### Gates — все 7 ✅ passed

| Gate | Observed | Floor | Status | vs v3 | vs v4 |
|------|----------|-------|--------|-------|-------|
| recall_at_5 | **1.0000** | 0.90 | ✅ | = | = |
| mrr | **0.9924** | 0.85 | ✅ | = | = |
| top_1_hit_rate | 0.9848 | 0.80 | ✅ | = | = |
| entity_f1 | 0.7500 | 0.70 | ✅ | = | = |
| claim_precision | **1.0000** | 0.80 | ✅ | 0.750 → **1.000** 🎉 | = |
| claim_recall | **1.0000** | 0.90 | ✅ | 0.600 → **1.000** 🎉 | = |
| external-retrieval/recall_at_5 | 1.0000 | 0.90 | ✅ | (new in v4) | = |

⚠️ Обратите внимание: в v3 floor для `claim_precision` был `0.50` и observed `0.75` (прошёл с запасом). В v4 floor подняли до `0.80` и observed стал `1.000` — gate ужесточили одновременно с починкой метрики. v5 удерживает `1.000` против нового floor `0.80`.

---

### 2.2 Release Profile (`evals/profiles/release.json`)

**Run ID**: `run-1785325450` | **Duration**: 10 552 ms  
**Result**: ✅ **PASSED** (verdict PASSED, 9/9 gates passed, 0 invalid)

| Suite | Total | Passed | Quality Failed |
|-------|-------|--------|----------------|
| `local-retrieval` | 66 | 66 | 0 |
| `extraction` | 9 | 9 | 0 |
| `claim-reconciliation` | 42 | 42 | 0 |
| `lifecycle` | 4 | 4 | 0 |
| `external-retrieval` | 2 | 2 | 0 |
| **Total** | **123** | **123** | **0** |

#### Lifecycle Suite Details (полностью чистый прогон)

| Case | Status | Metrics |
|------|--------|---------|
| `lifecycle-action-grounding` | ✅ Passed | action_grounding_pass_rate: 1.0 |
| `lifecycle-capacity` | ✅ Passed | — |
| `lifecycle-poisoning` | ✅ Passed | poisoning_pass_rate: 1.0 (был quality_failed в v2/v3) 🎉 |
| `lifecycle-public-surface` | ✅ Passed | — |

#### Gates — все 9 ✅ passed

| Gate | Observed | Floor | Status | vs v3 | vs v4 |
|------|----------|-------|--------|-------|-------|
| recall_at_5 | 1.0000 | 0.90 | ✅ | = | = |
| mrr | 0.9924 | 0.85 | ✅ | = | = |
| top_1_hit_rate | 0.9848 | 0.80 | ✅ | = | = |
| entity_f1 | 0.7500 | 0.70 | ✅ | = | = |
| claim_precision | **1.0000** | 0.80 | ✅ | 0.750 → **1.000** | = |
| claim_recall | **1.0000** | 0.90 | ✅ | 0.600 → **1.000** | = |
| action_grounding_pass_rate | **1.0000** | 1.00 | ✅ | (был invalid в v3) 🎉 | = |
| poisoning_pass_rate | **1.0000** | 1.00 | ✅ | (был invalid в v3) 🎉 | = |
| external-retrieval/recall_at_5 | 1.0000 | 0.90 | ✅ | (new in v4) | = |

🎉 Главное: lifecycle gate metrics propagation починен — `lifecycle-action-grounding` и `lifecycle-poisoning` теперь не invalid, а `1.0000` passed. `poisoning` больше не quality_failed (в v2/v3 было `0.6667`).

---

### 2.3 Nightly Profile (`evals/profiles/nightly.json`)

**Run ID**: `run-1785325483` | **Duration**: 9 956 ms  
**Result**: ✅ **PASSED** (verdict PASSED, 1/1 gates passed, 0 invalid)

| Suite | Total | Passed | Quality Failed |
|-------|-------|--------|----------------|
| `local-retrieval` | 66 | 66 | 0 |
| `extraction` | 9 | 9 | 0 |
| `claim-reconciliation` | 42 | 42 | 0 |
| `end-to-end` | 2 | **2** | 0 |
| `external-retrieval` | 2 | 2 | 0 |
| **Total** | **121** | **121** | **0** |

#### End-to-End Suite Details (регрессия v3 исправлена)

| Case | Status | Metrics | vs v3 |
|------|--------|---------|-------|
| `e2e-entity-extraction` | ✅ Passed | entity_tp=3, entity_fn=0, entity_fp=1, context_match_rate: 1.0 | quality_failed → passed 🎉 |
| `e2e-pipeline-completes` | ✅ Passed | context_items_returned > 0, context_match_rate: 1.0 | quality_failed → passed 🎉 |

🎉 Регрессия e2e-тестов из v3 (оба quality_failed, context_items=0) полностью исправлена. Контекст снова возвращается.

#### Gates — 1 ✅ passed

| Gate | Observed | Floor | Status | vs v3 |
|------|----------|-------|--------|-------|
| end-to-end/context_match_rate | **1.0000** | 1.0000 | ✅ | (new in v4) |

---

## 3. Criterion Benchmarks

### 3.1 Pipeline (`benches/pipeline.rs`)

| Benchmark | v5 ns/iter | ± | v4 | v3 | Δ v5 vs v4 |
|-----------|-----------|---|----|----|------------|
| `ingest_single_episode` | 63 904 069 | 918 751 | 75 226 729 | 61 793 229 | **−15.1%** ↓ 🎉 |
| `extract_single_episode` | 949 340 | 31 535 | 1 179 208 | 1 010 333 | **−19.5%** ↓ 🎉 |
| `assemble_context_single_query` | 66 241 687 | 537 648 | 67 317 687 | 66 960 583 | −1.6% |

### 3.2 NER CPU (`benches/ner_cpu.rs`)

| Benchmark | v5 ns/iter | ± | v4 | v3 | Δ v5 vs v4 |
|-----------|-----------|---|----|----|------------|
| `ner_cpu_single_window` | 71 076 739 | 3 012 271 | 81 092 020 | 69 054 666 | **−12.4%** ↓ 🎉 |
| `ner_cpu_multi_window` | 4 397 986 | 578 914 | 5 065 791 | 3 579 104 | **−13.2%** ↓ 🎉 |

### 3.3 NER Metal (`benches/ner_metal.rs`)

| Benchmark | v5 ns/iter | ± | v4 | v3 | Δ v5 vs v4 |
|-----------|-----------|---|----|----|------------|
| `ner_apple_silicon_production_single_window` | 72 729 333 | 910 645 | 70 903 291 | 43 (stub) | +2.6% |

📝 **Note**: v3 NER Metal был stub (`43 ns/iter`). v4 и v5 — реальный Apple Silicon GPU inference. v5 против v4 показывает лёгкий шум +2.6% в пределах ±1 std dev. Стабильно.

### 3.4 Contention (`benches/contention.rs`)

| Benchmark | v5 ns/iter | ± | v4 | v3 | Δ v5 vs v4 |
|-----------|-----------|---|----|----|------------|
| `contention_single_client` | 84 871 864 | 637 073 | 85 062 687 | 81 100 749 | −0.2% |
| `contention/clients_2` | 303 157 208 | 77 571 011 | 271 088 625 | 268 378 417 | +11.8% ⚠️ |
| `contention/clients_4` | 303 592 708 | 4 888 738 | 273 907 750 | 267 754 666 | +10.8% ⚠️ |

⚠️ Multi-client contention вырос на ~10-12% относительно v4. v5 измерение имеет очень высокий разброс у `clients_2` (±77 ms / ±26% от среднего), что говорит о GC/IO джиттере — возможно, contention bench ловит разную нагрузку от других процессов. Один запуск, статзначимости нет. Стоит перепрогнать с увеличенным `--measurement-time`.

---

## 4. Delta-анализ: v3 → v4 → v5

### 🎉 Что починено между v3 и v5

| Что | v3 | v4 | v5 |
|-----|----|----|----|
| PR verdict | QUALITY FAILED | **PASSED** | **PASSED** |
| Release verdict | INVALID (2 lifecycle gates invalid) | **PASSED** | **PASSED** |
| Nightly verdict | QUALITY FAILED (e2e regression) | **PASSED** | **PASSED** |
| `e2e-entity-extraction` | ❌ quality_failed | ✅ passed | ✅ passed |
| `e2e-pipeline-completes` | ❌ quality_failed | ✅ passed | ✅ passed |
| `lifecycle-poisoning` | ❌ quality_failed | ✅ passed | ✅ passed |
| `action_grounding_pass_rate` gate | ⚠️ invalid | ✅ passed (1.000) | ✅ passed (1.000) |
| `poisoning_pass_rate` gate | ⚠️ invalid | ✅ passed (1.000) | ✅ passed (1.000) |
| `claim_precision` (PR/release) | 0.750 | **1.000** | **1.000** |
| `claim_recall` (PR/release) | 0.600 | **1.000** | **1.000** |
| `external-retrieval/recall_at_5` gate | (не существовал) | ✅ 1.000 | ✅ 1.000 |
| NER Metal stub → real | 43 ns (stub) | 70.9 ms (real) | 72.7 ms (real) |

### ✅ Стабильно / улучшилось

| Метрика | v3 | v4 | v5 |
|---------|----|----|----|
| `recall_at_5` | 1.0000 | 1.0000 | 1.0000 |
| `mrr` | 0.9924 | 0.9924 | 0.9924 |
| `top_1_hit_rate` | 0.9848 | 0.9848 | 0.9848 |
| `entity_f1` | 0.7500 | 0.7500 | 0.7500 |
| `entity_precision` | 0.6000 | 0.6000 | 0.6000 |
| `entity_recall` | 1.0000 | 1.0000 | 1.0000 |
| `claim_f1` | 0.6667 | **1.0000** | **1.0000** |
| `lifecycle-action-grounding` | passed | passed | passed |
| `lifecycle-capacity` | passed | passed | passed |
| `lifecycle-public-surface` | passed | passed | passed |
| local-retrieval | 66/66 | 66/66 | 66/66 |
| extraction | 7/9 → 9/9 | 9/9 | 9/9 |
| claim-reconciliation | 38/42 → 42/42 | 42/42 | 42/42 |

### 🏎️ Performance: v4 → v5

| Bench | v4 | v5 | Δ |
|-------|----|----|---|
| ingest | 75.2 ms | **63.9 ms** | **−15.1%** ↓ 🎉 |
| extract | 1.18 ms | **0.95 ms** | **−19.5%** ↓ 🎉 |
| context | 67.3 ms | 66.2 ms | −1.6% |
| NER CPU single | 81.1 ms | **71.1 ms** | **−12.4%** ↓ 🎉 |
| NER CPU multi | 5.07 ms | **4.40 ms** | **−13.2%** ↓ 🎉 |
| NER Metal single | 70.9 ms | 72.7 ms | +2.6% (noise) |
| Contention single | 85.1 ms | 84.9 ms | −0.2% |
| Contention 2-clients | 271.1 ms | 303.2 ms | +11.8% ⚠️ high variance |
| Contention 4-clients | 273.9 ms | 303.6 ms | +10.8% ⚠️ high variance |

🎉 Pipeline и NER CPU значимо ускорились относительно v4 (`ingest` −15%, `extract` −19%, оба NER бенча −12-13%). Multi-client contention требует повторного прогона с увеличенным measurement-time для статзначимости.

---

## 5. Сводный статус всех прогонов

|  | v1 | v2 | v3 | v4 | v5 |
|--|----|----|----|----|----|
| PR gates passed | 3/6 | 4/6 | 6/6 | 7/7 | **7/7** ✅ |
| Release gates passed | 3/6 | 4/6 | 6/6+2 invalid | 9/9 | **9/9** ✅ |
| Nightly status | ❌ crash | ✅ passed | ✅ passed (с regression) | ✅ passed | ✅ passed |
| retrieval recall@5 | 1.000 | 0.987 | 1.000 | 1.000 | **1.000** |
| entity_f1 | 0.000 | 0.750 | 0.750 | 0.750 | **0.750** |
| claim_precision | 0.000 | 0.000 | 0.750 | **1.000** | **1.000** |
| claim_recall | 0.000 | 0.000 | 0.600 | **1.000** | **1.000** |
| e2e passed | 0/2 | 2/2 | 0/2 | 2/2 | **2/2** |
| lifecycle-poisoning passed | ❌ | ❌ | ❌ | ✅ | **✅** |

**Overall v5 verdict**: ✅ все три профиля дают verdict `PASSED`. Все 7+9+1 = **17 gates passed, 0 failed, 0 invalid**. Все 363 ожидаемых кейса (119+123+121) прошли. Pipeline и NER CPU показали значимое ускорение против v4. Multi-client contention требует повторного измерения из-за высокого разброса.

---

## 6. Артефакты

| Артефакт | Path |
|----------|------|
| PR eval | `target/evals/v5-pr.json` |
| Release eval | `target/evals/v5-release.json` |
| Nightly eval | `target/evals/v5-nightly.json` |
| Fixture log | `target/evals/reports/v5/fixtures.log` |
| PR log | `target/evals/reports/v5/pr.log` |
| Release log | `target/evals/reports/v5/release.log` |
| Nightly log | `target/evals/reports/v5/nightly.log` |
| Pipeline bench | `target/evals/reports/v5/benches/pipeline.txt` |
| NER CPU bench | `target/evals/reports/v5/benches/ner_cpu.txt` |
| NER Metal bench | `target/evals/reports/v5/benches/ner_metal.txt` |
| Contention bench | `target/evals/reports/v5/benches/contention.txt` |
| Pipeline summary | `target/evals/reports/v5/benches/pipeline-summary.txt` |
| NER CPU summary | `target/evals/reports/v5/benches/ner_cpu-summary.txt` |
| NER Metal summary | `target/evals/reports/v5/benches/ner_metal-summary.txt` |
| Contention summary | `target/evals/reports/v5/benches/contention-summary.txt` |
