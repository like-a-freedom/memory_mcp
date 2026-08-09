# NER Extractor Evaluation Comparison Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Cover every `NER_EXTRACTOR` backend (regex, anno, anno-onnx, classic GLiNER, VAGO LFM2) with comparable quality and performance benchmarks so a user can pick the right extractor for their scenario.

**Architecture:** Add (1) a shared `ner_fixtures` module in `eval-harness` that builds any extractor through the production constructors — `create_entity_extractor` for the lightweight kinds and anno-onnx (explicit `cache_dir`), and the store-free `GlinerEntityExtractor::new` / `VagoLfm2EntityExtractor::new_with_runtime` for the model-backed kinds (fixture-gated, offline, never a download), (2) one `NerQualitySuite` instance per extractor that scores a single shared RU/EN/mixed corpus (`evals/corpora/ner/ner_quality.json`) and reports mention-level precision/recall/F1 through the existing `ClassificationReducer` + report machinery, and (3) full Criterion bench coverage (cold start + single/multi window latency) for all five extractors in `ner_cpu.rs`. A new manual-only profile `ner_quality` ties the suites together; CI profiles are untouched (model checkpoints are gitignored and absent in CI).

**Tech Stack:** Rust, `eval-harness` crate (`memory-eval` binary), `memory_mcp::service` (`create_entity_extractor`, `GlinerEntityExtractor::new`, `VagoLfm2EntityExtractor::new_with_runtime`) + `EntityExtractor` trait, `ClassificationReducer` / `render_case_metrics`, Criterion, serde_json. No new dependencies.

## Global Constraints

- **Closed NER catalog**: `NER_EXTRACTOR` = `anno` | `regex` | `anno-onnx` | `urchade/gliner_multi-v2.1` | `VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER`. Suite ids mirror these: `ner-quality-anno`, `ner-quality-regex`, `ner-quality-anno-onnx`, `ner-quality-gliner`, `ner-quality-vago`.
- **Fixture-gating**: model checkpoints live under `crates/memory-mcp/tests/models/ner/` (gitignored, absent in CI). Anything that needs a checkpoint is either fixture-conditional (`Option`/`None` skip) or `#[ignore]`d. `anno` and `regex` must always work offline.
- **Verdict semantics** (`derive_run_verdict`): any `CaseStatus::Invalid` outcome ⇒ whole run `Invalid` (exit 2). Therefore the `ner_quality` profile is manual-only and users select present fixtures via `--suite` filters. Never add NER-quality suites to `pr.json`/`release.json`/`nightly.json`.
- **ADR-0025 guard**: gate-consumed metric keys (`entity_mention_f1`) must come from `render_case_metrics` + `ClassificationReducer` with the canonical prefix `entity_mention` (the same one the existing `extraction.rs` suite uses), never from literal `metrics.insert("entity_mention_f1", ...)`. Suite-local diagnostics (e.g. `ner_typed_f1`) may be inserted literally (precedent: `fact_type_accuracy` in `extraction.rs`).
- **Matching convention**: mention matching is case-insensitive on `EntityCandidate::canonical_name` (extractors normalize differently). Corpus spans are character offsets into `text` (Python reference convention) and are validated by round-trip, never compared to extractor output (`EntityCandidate` carries no spans).
- **Zero model downloads in eval paths**: `ner_fixtures::build_extractor` only uses local prepared checkpoints through the production **store-free** constructors (`create_entity_extractor` with an explicit `cache_dir` for anno-onnx, `GlinerEntityExtractor::new`, `VagoLfm2EntityExtractor::new_with_runtime`). No HF revision resolve or download in benches or suites; a missing or incomplete fixture yields `None`, never a panic or a download.
- **No new dependencies; no production-crate changes.** All work lives in `crates/eval-harness` and `evals/`.
- **Quality bar**: `cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings`, `cargo fmt --all --check`, `cargo test --workspace --lib --bins --tests --locked` must stay green (CI ignores `#[ignore]`d tests).
- **Threshold**: model-backed extractors benchmark/evaluate at `threshold = 0.5`, labels = `person,company,location,product,event,technology` (matching the corpus labels).

---

## Background (investigation findings)

**Five extractors today** (all behind `create_entity_extractor` in `crates/memory-mcp/src/service/entity_extraction.rs`):

| Selector | Kind | Model / fixture | Labels | Build path |
|---|---|---|---|---|
| `anno` | Anno NuNER rules | none (offline) | `map_label` (PER→person) | direct |
| `regex` | project regex | none (offline) | `classify_entity_type` | direct |
| `anno-onnx` | NuNER ONNX (CPU) | `tests/models/ner/deepanwa--NuNerZero_onnx/{model.onnx,tokenizer.json,config.json}` (~1.85 GB) | config labels | direct `cache_dir` fixture |
| `urchade/gliner_multi-v2.1` | candle DeBERTa | `tests/models/ner/urchade--gliner_multi-v2.1/` (~1.1 GB) | config labels | seeded artifact store |
| `VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER` | candle LFM2 | `tests/models/ner/VAGOsolutions--SauerkrautLM-LFM2.5-GLiNER/pytorch_model.bin` (~1.6 GB) | config labels | direct `cache_dir` fixture |

**Existing eval assets:**
- `crates/eval-harness/benches/ner_cpu.rs` — Criterion latency for gliner + vago only; `ner_metal.rs` — Apple Silicon (gliner/VAGO). `anno`, `regex`, `anno-onnx` have **no** latency bench. Texts are hardcoded in `eval_harness::benchmark::NerBenchmarkFixture`.
- `evals/corpora/ner/` — `vago_runtime_regression.json` (structural gate) and `vago_release_parity.json` (span+score, pending Python reference). VAGO-only; not usable as a cross-extractor quality corpus.
- `eval-harness` suites — `extraction` measures entity sets through the *default configured* extractor only (service E2E). No suite is parameterized over extractors.
- `EvalProfile` is a closed enum `Pr | Release | Nightly | ResponseSize`; a new profile name requires a new variant.
- `RunContext` carries only `profile`; suites are wired in `main.rs` via `match suite_decl.id.as_str()`.
- `crates/memory-mcp/tests/models/` is gitignored; CI runs `cargo test --workspace --lib --bins --tests --locked` (no `--ignored`).

**Gaps vs. today's state this plan closes:** no uniform quality corpus for all extractors; no quality suite parameterized over extractors; no latency coverage for anno/regex/anno-onnx. The current `extraction` suite (end-to-end, default-configured anno) stays as-is — this plan adds *per-backend* quality/latency comparison, not a change to end-to-end coverage.

**Design decisions (DRY / KISS / DDD / SOLID):**
1. `ner_fixtures.rs` is the single source of truth for fixture paths, default labels, and `build_extractor(kind)` — benches and the quality suite share it (DRY, SRP).
2. `NerQualitySuite` depends on the `EntityExtractor` abstraction (DIP), takes an explicit suite id + kind, and reuses the existing `ClassificationReducer`/`render_case_metrics`/report machinery (KISS, no new metrics pipeline).
3. Quality = mention-level F1 (fair across type-vocabulary differences: regex has no `product`, anno maps labels through `map_label`). Typed match is a per-case diagnostic only.
4. Model-backed suites emit explicit `Invalid` outcomes when the checkpoint is missing (honest, deterministic) — and the manual profile workflow filters to available fixtures with `--suite`.

---

## File Structure

| File | Responsibility |
|---|---|
| Create `evals/corpora/ner/ner_quality.json` | Shared RU/EN/mixed NER quality corpus (10 cases, 46 annotated entities) |
| Create `crates/eval-harness/tests/ner_quality_corpus.rs` | Offline structural validation of the corpus + consistency with `vago_release_parity.json` |
| Create `crates/eval-harness/src/ner_fixtures.rs` | Fixture paths, `default_labels()`, async `build_extractor(kind) -> Option<Arc<dyn EntityExtractor>>` |
| Create `crates/eval-harness/src/suites/ner_quality.rs` | `NerQualitySuite` + `register(suite_id, suites)` + `run_case` + unit tests; gate-consumed `entity_mention_*` metrics from `ClassificationReducer`, suite-local diagnostic `ner_typed_f1` |
| Modify `crates/eval-harness/src/suites.rs` | `pub mod ner_quality;` re-export + `SUITE_FILES` guard list |
| Modify `crates/eval-harness/src/lib.rs` | `pub mod ner_fixtures;` |
| Modify `crates/eval-harness/src/domain.rs` | Add `EvalProfile::NerQuality` + serialization test |
| Create `evals/profiles/ner_quality.json` | Manual comparison profile (5 suites, no gates; `expected_coverage` always set — it's mandatory for non-`nightly` profiles) |
| Modify `crates/eval-harness/src/main.rs` | Wire 5 `ner-quality-*` suite ids |
| Modify `crates/eval-harness/benches/ner_cpu.rs` | All-extractor latency benches via `ner_fixtures`; cold-start notes |
| Create `crates/eval-harness/tests/ner_quality_real_models.rs` | `#[ignore]`d fixture-gated real-GLiNER run against the corpus |
| Modify `docs/agent/EVALUATION.md` | "NER extractor comparison" section |
| Create `docs/adr/0037-ner-eval-comparison.md` | Records the comparison-tool decision |

---

### Task 1: Add `EvalProfile::NerQuality`

**Files:**
- Modify: `crates/eval-harness/src/domain.rs:219-225`
- Test: same file, `eval_profile_serializes_with_snake_case` (~L381-390)

**Interfaces:**
- Produces: `EvalProfile::NerQuality`, serialized as `"ner_quality"` — consumed by `evals/profiles/ner_quality.json` and `RunContext.profile`.

- [ ] **Step 1: Write the failing test**

In `crates/eval-harness/src/domain.rs`, extend the existing serialization test:

```rust
fn eval_profile_serializes_with_snake_case() {
    assert_eq!(serde_json::to_string(&EvalProfile::Pr).unwrap(), "\"pr\"");
    assert_eq!(
        serde_json::to_string(&EvalProfile::Release).unwrap(),
        "\"release\""
    );
    assert_eq!(
        serde_json::to_string(&EvalProfile::Nightly).unwrap(),
        "\"nightly\""
    );
    assert_eq!(
        serde_json::to_string(&EvalProfile::ResponseSize).unwrap(),
        "\"response_size\""
    );
    assert_eq!(
        serde_json::to_string(&EvalProfile::NerQuality).unwrap(),
        "\"ner_quality\""
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p eval-harness eval_profile_serializes_with_snake_case`
Expected: FAIL — compile error, `EvalProfile::NerQuality` not found.

- [ ] **Step 3: Add the variant**

```rust
pub enum EvalProfile {
    Pr,
    Release,
    Nightly,
    ResponseSize,
    NerQuality,
}
```

- [ ] **Step 4: Run test to verify it passes**

Run: `cargo test -p eval-harness eval_profile_serializes_with_snake_case`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/eval-harness/src/domain.rs
git commit -m "feat(eval): add NerQuality eval profile variant"
```

---

### Task 2: Unified NER quality corpus + structural validation

**Files:**
- Create: `evals/corpora/ner/ner_quality.json`
- Create: `crates/eval-harness/tests/ner_quality_corpus.rs`

**Interfaces:**
- Produces: JSON at `crates/eval-harness/../../evals/corpora/ner/ner_quality.json` (resolved via `CARGO_MANIFEST_DIR`), schema `{ schema_version, fixture_status, languages, purpose, cases: [{id, language, text, labels, entities: [{start, end, text, label}]}] }` — consumed by Task 4's suite and Task 7's integration test.

**Corpus design rationale (KISS / avoid bias):** 46 entities over 10 cases (~4.6 per case) is enough for a *directional* mention-F1 comparison but too small for tight confidence intervals; the corpus is explicitly a regression/selection aid, not a benchmark leaderboard. The 6 reused VAGO cases keep one source of truth; the 4 new cases widen coverage to RU product/location nuances and mixed-script person+company. Label vocabulary is the shared `person/company/location/product/event/technology` set so every model-backed backend is asked the same labels; lightweight backends use their own type classifiers and only mention text is scored.

- [ ] **Step 1: Write the failing structural test**

Create `crates/eval-harness/tests/ner_quality_corpus.rs`:

```rust
//! Offline structural validation of the shared NER quality corpus.

use std::path::PathBuf;

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/corpora/ner/ner_quality.json")
}

#[derive(serde::Deserialize)]
struct CorpusFile {
    schema_version: u32,
    fixture_status: String,
    #[serde(default)]
    languages: Vec<String>,
    cases: Vec<CorpusCase>,
}

#[derive(serde::Deserialize)]
struct CorpusCase {
    id: String,
    language: String,
    text: String,
    labels: Vec<String>,
    #[serde(default)]
    entities: Vec<CorpusEntity>,
}

#[derive(serde::Deserialize)]
struct CorpusEntity {
    start: usize,
    end: usize,
    text: String,
    label: String,
}

fn load() -> CorpusFile {
    let raw = std::fs::read_to_string(&corpus_path())
        .unwrap_or_else(|err| panic!("read corpus {}: {err}", corpus_path().display()));
    serde_json::from_str(&raw)
        .unwrap_or_else(|err| panic!("parse corpus {}: {err}", corpus_path().display()))
}

#[test]
fn ner_quality_corpus_is_structurally_valid() {
    let corpus = load();
    assert_eq!(corpus.schema_version, 1);
    assert_eq!(corpus.fixture_status, "official");
    assert!(!corpus.cases.is_empty(), "corpus must not be empty");

    let mut seen = std::collections::HashSet::new();
    for case in &corpus.cases {
        assert!(
            seen.insert(case.id.clone()),
            "duplicate case id {}",
            case.id
        );
        assert!(
            corpus.languages.contains(&case.language),
            "case {} language {} not declared in languages {:?}",
            case.id,
            case.language,
            corpus.languages
        );
        assert!(!case.labels.is_empty(), "case {} has no labels", case.id);
        assert!(!case.entities.is_empty(), "case {} has no entities", case.id);
        for entity in &case.entities {
            let actual: String = case
                .text
                .chars()
                .skip(entity.start)
                .take(entity.end.saturating_sub(entity.start))
                .collect();
            assert_eq!(
                actual, entity.text,
                "case {}: span [{}, {}) does not round-trip `{}` (got `{actual}`)",
                case.id, entity.start, entity.end, entity.text
            );
            assert!(
                case.labels.contains(&entity.label),
                "case {}: entity label `{}` not in ordered labels {:?}",
                case.id,
                entity.label,
                case.labels
            );
        }
    }
}

#[test]
fn shared_cases_match_vago_release_parity_corpus() {
    // The six VAGO-parity cases are reused verbatim (ids prefixed `q-`) so the
    // two corpora cannot drift. Pin text + span + label per shared id.
    let parity_path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/corpora/ner/vago_release_parity.json");
    let raw = std::fs::read_to_string(&parity_path)
        .unwrap_or_else(|err| panic!("read {}: {err}", parity_path.display()));
    #[derive(serde::Deserialize)]
    struct ParityFile {
        cases: Vec<ParityCase>,
    }
    #[derive(serde::Deserialize)]
    struct ParityCase {
        id: String,
        text: String,
        entities: Vec<ParityEntity>,
    }
    #[derive(serde::Deserialize)]
    struct ParityEntity {
        start: usize,
        end: usize,
        text: String,
        label: String,
    }
    let parity: ParityFile = serde_json::from_str(&raw).expect("parse parity corpus");

    let corpus = load();
    for parity_case in &parity.cases {
        let shared = corpus
            .cases
            .iter()
            .find(|case| case.id == format!("q-{}", parity_case.id))
            .unwrap_or_else(|| panic!("corpus is missing shared case q-{}", parity_case.id));
        assert_eq!(shared.text, parity_case.text, "case q-{}: text drift", parity_case.id);
        assert_eq!(shared.entities.len(), parity_case.entities.len());
        for (ours, theirs) in shared.entities.iter().zip(parity_case.entities.iter()) {
            assert_eq!(ours.start, theirs.start, "case q-{}: start drift", parity_case.id);
            assert_eq!(ours.end, theirs.end, "case q-{}: end drift", parity_case.id);
            assert_eq!(ours.text, theirs.text, "case q-{}: text drift", parity_case.id);
            assert_eq!(ours.label, theirs.label, "case q-{}: label drift", parity_case.id);
        }
    }
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p eval-harness --test ner_quality_corpus`
Expected: FAIL — corpus file missing (read panic).

- [ ] **Step 3: Create the corpus**

Create `evals/corpora/ner/ner_quality.json` with the exact content below (spans are 0-based character offsets; all six `q-*` cases are byte-identical to `vago_release_parity.json` content with renamed ids):

```json
{
  "schema_version": 1,
  "fixture_status": "official",
  "languages": ["ru", "en", "mixed"],
  "purpose": "Shared quality corpus for every NER_EXTRACTOR backend (regex, anno, anno-onnx, classic GLiNER, VAGO LFM2). Mention-level F1 compares canonical names; spans and labels are hand-annotated ground truth. The six q-* cases reuse vago_release_parity.json content verbatim so both corpora stay consistent.",
  "cases": [
    {
      "id": "q-ru-1",
      "language": "ru",
      "text": "Иван Петров работает в компании Яндекс в Москве.",
      "labels": ["person", "company", "location"],
      "entities": [
        {"start": 0, "end": 11, "text": "Иван Петров", "label": "person"},
        {"start": 32, "end": 38, "text": "Яндекс", "label": "company"},
        {"start": 41, "end": 47, "text": "Москве", "label": "location"}
      ]
    },
    {
      "id": "q-ru-2",
      "language": "ru",
      "text": "Мария Иванова представила продукт СберБизнес на конференции AI Journey в Санкт-Петербурге.",
      "labels": ["person", "product", "event", "location"],
      "entities": [
        {"start": 0, "end": 13, "text": "Мария Иванова", "label": "person"},
        {"start": 34, "end": 44, "text": "СберБизнес", "label": "product"},
        {"start": 60, "end": 70, "text": "AI Journey", "label": "event"},
        {"start": 73, "end": 89, "text": "Санкт-Петербурге", "label": "location"}
      ]
    },
    {
      "id": "q-en-1",
      "language": "en",
      "text": "Alice Smith from OpenAI presented the Surface Laptop 6 at Build 2026 in Seattle.",
      "labels": ["person", "company", "product", "event", "location"],
      "entities": [
        {"start": 0, "end": 11, "text": "Alice Smith", "label": "person"},
        {"start": 17, "end": 23, "text": "OpenAI", "label": "company"},
        {"start": 38, "end": 54, "text": "Surface Laptop 6", "label": "product"},
        {"start": 58, "end": 68, "text": "Build 2026", "label": "event"},
        {"start": 72, "end": 79, "text": "Seattle", "label": "location"}
      ]
    },
    {
      "id": "q-en-2",
      "language": "en",
      "text": "At Cloud Summit 2026 in Berlin, Bob Jones and DeepMind compared Pixel 8 Pro with PostgreSQL.",
      "labels": ["person", "company", "product", "event", "location", "technology"],
      "entities": [
        {"start": 3, "end": 20, "text": "Cloud Summit 2026", "label": "event"},
        {"start": 24, "end": 30, "text": "Berlin", "label": "location"},
        {"start": 32, "end": 41, "text": "Bob Jones", "label": "person"},
        {"start": 46, "end": 54, "text": "DeepMind", "label": "company"},
        {"start": 64, "end": 75, "text": "Pixel 8 Pro", "label": "product"},
        {"start": 81, "end": 91, "text": "PostgreSQL", "label": "technology"}
      ]
    },
    {
      "id": "q-mixed-1",
      "language": "mixed",
      "text": "Иван Петров from Microsoft unveiled Surface Laptop 6 at Build 2026 in Seattle using Kubernetes.",
      "labels": ["person", "company", "product", "event", "location", "technology"],
      "entities": [
        {"start": 0, "end": 11, "text": "Иван Петров", "label": "person"},
        {"start": 17, "end": 26, "text": "Microsoft", "label": "company"},
        {"start": 36, "end": 52, "text": "Surface Laptop 6", "label": "product"},
        {"start": 56, "end": 66, "text": "Build 2026", "label": "event"},
        {"start": 70, "end": 77, "text": "Seattle", "label": "location"},
        {"start": 84, "end": 94, "text": "Kubernetes", "label": "technology"}
      ]
    },
    {
      "id": "q-mixed-2",
      "language": "mixed",
      "text": "Наталья Смирнова of Yandex Cloud demoed сервис YandexGPT на AI Day в Москве.",
      "labels": ["person", "company", "product", "event", "location"],
      "entities": [
        {"start": 0, "end": 16, "text": "Наталья Смирнова", "label": "person"},
        {"start": 20, "end": 32, "text": "Yandex Cloud", "label": "company"},
        {"start": 47, "end": 56, "text": "YandexGPT", "label": "product"},
        {"start": 60, "end": 66, "text": "AI Day", "label": "event"},
        {"start": 69, "end": 75, "text": "Москве", "label": "location"}
      ]
    },
    {
      "id": "q-en-3",
      "language": "en",
      "text": "Elon Musk announced that Tesla will expand in Texas and SpaceX hired Anna Petrova as lead engineer.",
      "labels": ["person", "company", "location"],
      "entities": [
        {"start": 0, "end": 9, "text": "Elon Musk", "label": "person"},
        {"start": 25, "end": 30, "text": "Tesla", "label": "company"},
        {"start": 46, "end": 51, "text": "Texas", "label": "location"},
        {"start": 56, "end": 62, "text": "SpaceX", "label": "company"},
        {"start": 69, "end": 81, "text": "Anna Petrova", "label": "person"}
      ]
    },
    {
      "id": "q-en-4",
      "language": "en",
      "text": "Microsoft shipped the Surface Laptop 6 with PostgreSQL support at Build 2026.",
      "labels": ["company", "product", "technology", "event"],
      "entities": [
        {"start": 0, "end": 9, "text": "Microsoft", "label": "company"},
        {"start": 22, "end": 38, "text": "Surface Laptop 6", "label": "product"},
        {"start": 44, "end": 54, "text": "PostgreSQL", "label": "technology"},
        {"start": 66, "end": 76, "text": "Build 2026", "label": "event"}
      ]
    },
    {
      "id": "q-ru-3",
      "language": "ru",
      "text": "Иван Иванов посетил конференцию Yandex Day в Москве и рассказал о продукте YaDisk.",
      "labels": ["person", "event", "location", "product"],
      "entities": [
        {"start": 0, "end": 11, "text": "Иван Иванов", "label": "person"},
        {"start": 32, "end": 42, "text": "Yandex Day", "label": "event"},
        {"start": 45, "end": 51, "text": "Москве", "label": "location"},
        {"start": 75, "end": 81, "text": "YaDisk", "label": "product"}
      ]
    },
    {
      "id": "q-mixed-3",
      "language": "mixed",
      "text": "Сергей Козлов joined Acme Corp in Berlin and leads разработку платформы DataHub.",
      "labels": ["person", "company", "location", "technology"],
      "entities": [
        {"start": 0, "end": 13, "text": "Сергей Козлов", "label": "person"},
        {"start": 21, "end": 30, "text": "Acme Corp", "label": "company"},
        {"start": 34, "end": 40, "text": "Berlin", "label": "location"},
        {"start": 72, "end": 79, "text": "DataHub", "label": "technology"}
      ]
    }
  ]
}
```

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p eval-harness --test ner_quality_corpus`
Expected: PASS (both tests).

- [ ] **Step 5: Commit**

```bash
git add evals/corpora/ner/ner_quality.json crates/eval-harness/tests/ner_quality_corpus.rs
git commit -m "feat(eval): add shared NER quality corpus with structural validation"
```

---

### Task 3: Shared `ner_fixtures` module

**Files:**
- Create: `crates/eval-harness/src/ner_fixtures.rs`
- Modify: `crates/eval-harness/src/lib.rs:1-16`

**Interfaces:**
- Consumes: `memory_mcp::config::{GlinerDeviceKind, NerConfig, NerExtractorConfig, NerExtractorKind}`, `memory_mcp::logging::StdoutLogger`, `memory_mcp::service::{EntityExtractor, GlinerEntityExtractor, VagoLfm2EntityExtractor, create_entity_extractor}`.
- Produces:
  - `pub fn fixture_root() -> PathBuf`
  - `pub fn default_labels() -> Vec<String>`
  - `pub async fn build_extractor(kind: NerExtractorKind) -> Option<Arc<dyn EntityExtractor>>` — `Some` for `anno`/`regex` always; model kinds only when their local fixture exists; `None` otherwise.
  - `pub fn fixture_present(kind: NerExtractorKind) -> bool`

- [ ] **Step 1: Write the failing tests (in-module)**

Add to `crates/eval-harness/src/ner_fixtures.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_root_points_at_memory_mcp_model_dir() {
        assert!(fixture_root().ends_with("memory-mcp/tests/models/ner"));
    }

    #[test]
    fn default_labels_cover_corpus_labels() {
        let labels = default_labels();
        for required in ["person", "company", "location", "product", "event", "technology"] {
            assert!(labels.iter().any(|l| l == required), "missing {required}");
        }
    }

    #[test]
    fn lightweight_kinds_build_without_fixtures() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        for kind in [NerExtractorKind::Anno, NerExtractorKind::Regex] {
            let extractor = rt.block_on(build_extractor(kind));
            assert!(extractor.is_some(), "{kind:?} must build offline");
        }
    }

    #[test]
    fn model_kinds_are_fixture_gated() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        for kind in [
            NerExtractorKind::AnnoOnnx,
            NerExtractorKind::ClassicGliner,
            NerExtractorKind::SauerkrautLfm25,
        ] {
            let extractor = rt.block_on(build_extractor(kind));
            assert_eq!(fixture_present(kind), extractor.is_some(), "{kind:?}");
        }
    }

    #[test]
    fn model_kinds_declare_required_checkpoint_files() {
        // The completeness contract that keeps `build_extractor` panic-free:
        // every model-backed kind declares the exact files its loader reads.
        assert_eq!(
            required_files(NerExtractorKind::AnnoOnnx),
            &["model.onnx", "tokenizer.json", "config.json"]
        );
        assert_eq!(
            required_files(NerExtractorKind::ClassicGliner),
            &["model.safetensors", "gliner_config.json", "tokenizer.json"]
        );
        assert_eq!(
            required_files(NerExtractorKind::SauerkrautLfm25),
            &["pytorch_model.bin", "gliner_config.json", "tokenizer.json"]
        );
        // Lightweight kinds never consult the filesystem.
        assert!(required_files(NerExtractorKind::Anno).is_empty());
        assert!(required_files(NerExtractorKind::Regex).is_empty());
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p eval-harness ner_fixtures`
Expected: FAIL — module/file not found (compile error) or tests missing.

- [ ] **Step 3: Implement the module**

Create `crates/eval-harness/src/ner_fixtures.rs`:

```rust
//! Shared NER fixture resolution for benches and evaluation suites.
//!
//! Single source of truth for where local NER checkpoints live, the default
//! label set, and how to build any `NER_EXTRACTOR` backend for benchmarking.
//!
//! Model-backed kinds use the production **store-free** constructors
//! (`GlinerEntityExtractor::new`, `VagoLfm2EntityExtractor::new_with_runtime`,
//! `create_entity_extractor` with an explicit `cache_dir` for anno-onnx) so
//! evaluation never resolves upstream revisions or downloads checkpoints.
//! Kinds are fixture-gated: `build_extractor` returns `None` when the local
//! checkpoint (or any required file) is absent, so benches and suites can
//! skip honestly.

use std::path::PathBuf;
use std::sync::Arc;

use memory_mcp::config::{GlinerDeviceKind, NerConfig, NerExtractorConfig, NerExtractorKind};
use memory_mcp::logging::StdoutLogger;
use memory_mcp::service::{
    EntityExtractor, GlinerEntityExtractor, VagoLfm2EntityExtractor, create_entity_extractor,
};

/// Root of the local (gitignored) NER checkpoints.
pub fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("memory-mcp")
        .join("tests")
        .join("models")
        .join("ner")
}

/// Required checkpoint files per model-backed kind.
fn required_files(kind: NerExtractorKind) -> &'static [&'static str] {
    match kind {
        NerExtractorKind::AnnoOnnx => &["model.onnx", "tokenizer.json", "config.json"],
        NerExtractorKind::ClassicGliner => {
            &["model.safetensors", "gliner_config.json", "tokenizer.json"]
        }
        NerExtractorKind::SauerkrautLfm25 => {
            &["pytorch_model.bin", "gliner_config.json", "tokenizer.json"]
        }
        NerExtractorKind::Anno | NerExtractorKind::Regex => &[],
    }
}

/// Fixture directory for a model-backed kind that is present and complete.
/// Lightweight kinds have no fixture directory (`None`).
fn fixture_dir(kind: NerExtractorKind) -> Option<PathBuf> {
    let dir = match kind {
        NerExtractorKind::AnnoOnnx => fixture_root().join("deepanwa--NuNerZero_onnx"),
        NerExtractorKind::ClassicGliner => fixture_root().join("urchade--gliner_multi-v2.1"),
        NerExtractorKind::SauerkrautLfm25 => {
            fixture_root().join("VAGOsolutions--SauerkrautLM-LFM2.5-GLiNER")
        }
        NerExtractorKind::Anno | NerExtractorKind::Regex => return None,
    };
    if dir.is_dir()
        && required_files(kind)
            .iter()
            .all(|file| dir.join(file).is_file())
    {
        Some(dir)
    } else {
        None
    }
}

/// Whether the checkpoint for `kind` exists locally. Lightweight kinds are
/// always "present".
pub fn fixture_present(kind: NerExtractorKind) -> bool {
    matches!(kind, NerExtractorKind::Anno | NerExtractorKind::Regex)
        || fixture_dir(kind).is_some()
}

/// Default label set shared by benches and the quality suite (matches the
/// corpus label vocabulary).
pub fn default_labels() -> Vec<String> {
    vec![
        "person".to_string(),
        "company".to_string(),
        "location".to_string(),
        "product".to_string(),
        "event".to_string(),
        "technology".to_string(),
    ]
}

fn logger() -> StdoutLogger {
    StdoutLogger::new("error")
}

/// Builds the extractor for `kind` from a local fixture, when present.
///
/// Returns `None` when a model-backed kind has no complete local fixture.
/// Model-backed kinds are built through the production store-free
/// constructors, so no revision is resolved and nothing is downloaded.
/// A `None` from a present-but-incomplete fixture is never a panic.
pub async fn build_extractor(kind: NerExtractorKind) -> Option<Arc<dyn EntityExtractor>> {
    let extractor = match kind {
        NerExtractorKind::Anno => {
            create_entity_extractor(
                &NerConfig {
                    extractor: NerExtractorConfig::Anno,
                },
                env!("CARGO_MANIFEST_DIR"),
                &logger(),
            )
            .await
            .expect("anno extractor must build")
        }
        NerExtractorKind::Regex => {
            create_entity_extractor(
                &NerConfig {
                    extractor: NerExtractorConfig::Regex,
                },
                env!("CARGO_MANIFEST_DIR"),
                &logger(),
            )
            .await
            .expect("regex extractor must build")
        }
        NerExtractorKind::AnnoOnnx => {
            // `create_entity_extractor` with an explicit `cache_dir` treats it
            // as a raw model directory (anno_onnx::build), so the ONNX fixture
            // is used directly with no store and no download.
            let Some(dir) = fixture_dir(kind) else {
                return None;
            };
            create_entity_extractor(
                &NerConfig {
                    extractor: NerExtractorConfig::AnnoOnnx(memory_mcp::config::ModelBackedNerConfig {
                        cache_dir: Some(dir),
                        labels: default_labels(),
                        threshold: Some(0.5),
                        max_concurrency: 1,
                        idle_unload_secs: 0,
                    }),
                },
                env!("CARGO_MANIFEST_DIR"),
                &logger(),
            )
            .await
            .expect("anno-onnx extractor must build from a prepared checkpoint")
        }
        NerExtractorKind::ClassicGliner => {
            let Some(dir) = fixture_dir(kind) else {
                return None;
            };
            // Store-free production constructor: direct lazy loader, no
            // revision resolution, no download.
            match GlinerEntityExtractor::new(&dir, default_labels(), 0.5) {
                Ok(extractor) => Arc::new(extractor),
                Err(_) => return None,
            }
        }
        NerExtractorKind::SauerkrautLfm25 => {
            let Some(dir) = fixture_dir(kind) else {
                return None;
            };
            // Store-free production constructor (same path the release-parity
            // gate uses): direct lazy loader over `pytorch_model.bin`.
            match VagoLfm2EntityExtractor::new_with_runtime(
                &dir,
                default_labels(),
                0.5,
                1,    // batch_size
                1536, // max_batch_tokens
                1,    // max_concurrency
                GlinerDeviceKind::Cpu,
                0, // idle_unload_secs (retain)
                logger(),
            ) {
                Ok(extractor) => Arc::new(extractor),
                Err(_) => return None,
            }
        }
    };
    Some(extractor)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixture_root_points_at_memory_mcp_model_dir() {
        assert!(fixture_root().ends_with("memory-mcp/tests/models/ner"));
    }

    #[test]
    fn default_labels_cover_corpus_labels() {
        let labels = default_labels();
        for required in ["person", "company", "location", "product", "event", "technology"] {
            assert!(labels.iter().any(|l| l == required), "missing {required}");
        }
    }

    #[test]
    fn lightweight_kinds_build_without_fixtures() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        for kind in [NerExtractorKind::Anno, NerExtractorKind::Regex] {
            let extractor = rt.block_on(build_extractor(kind));
            assert!(extractor.is_some(), "{kind:?} must build offline");
        }
    }

    #[test]
    fn model_kinds_are_fixture_gated() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        for kind in [
            NerExtractorKind::AnnoOnnx,
            NerExtractorKind::ClassicGliner,
            NerExtractorKind::SauerkrautLfm25,
        ] {
            let extractor = rt.block_on(build_extractor(kind));
            assert_eq!(fixture_present(kind), extractor.is_some(), "{kind:?}");
        }
    }

    #[test]
    fn model_kinds_declare_required_checkpoint_files() {
        // The completeness contract that keeps `build_extractor` panic-free:
        // every model-backed kind declares the exact files its loader reads.
        assert_eq!(
            required_files(NerExtractorKind::AnnoOnnx),
            &["model.onnx", "tokenizer.json", "config.json"]
        );
        assert_eq!(
            required_files(NerExtractorKind::ClassicGliner),
            &["model.safetensors", "gliner_config.json", "tokenizer.json"]
        );
        assert_eq!(
            required_files(NerExtractorKind::SauerkrautLfm25),
            &["pytorch_model.bin", "gliner_config.json", "tokenizer.json"]
        );
        // Lightweight kinds never consult the filesystem.
        assert!(required_files(NerExtractorKind::Anno).is_empty());
        assert!(required_files(NerExtractorKind::Regex).is_empty());
    }
}
```

- [ ] **Step 4: Register the module in `lib.rs`**

Add to `crates/eval-harness/src/lib.rs` (alphabetical order, after `merge`):

```rust
pub mod ner_fixtures;
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p eval-harness ner_fixtures`
Expected: PASS (lightweight kinds build; model kinds match `fixture_present`).

- [ ] **Step 6: Commit**

```bash
git add crates/eval-harness/src/ner_fixtures.rs crates/eval-harness/src/lib.rs
git commit -m "feat(eval): add shared NER fixture resolution and extractor builder"
```

---

### Task 4: `NerQualitySuite`

**Files:**
- Create: `crates/eval-harness/src/suites/ner_quality.rs`
- Modify: `crates/eval-harness/src/suites.rs:1-59`

**Interfaces:**
- Consumes: `ner_fixtures::{build_extractor, fixture_present}`, `EntityExtractor`, corpus JSON from Task 2.
- Produces:
  - `pub struct NerQualitySuite` implementing `EvalSuite` with dynamic id.
  - `pub fn register(suite_id: &str, suites: &mut Vec<Box<dyn EvalSuite>>) -> Result<(), EvalError>` — called from `main.rs`; maps `ner-quality-*` → `NerExtractorKind`, builds one suite (extractor built lazily inside `run()`); returns the corpus-load error for the caller to record as a `RunIssue`.
  - Metrics: `entity_mention_precision`, `entity_mention_recall`, `entity_mention_f1` (reducer prefix `entity_mention`, matching the existing `extraction.rs` convention); per-case diagnostic `ner_typed_f1`.
  - Case ids = corpus case ids; expected coverage = corpus size.

- [ ] **Step 1: Write the failing tests**

In `crates/eval-harness/src/suites/ner_quality.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use memory_mcp::service::EntityExtractor;
    use std::sync::Arc;

    /// Scripted extractor: returns a fixed candidate list per case text.
    #[derive(Clone)]
    struct FakeExtractor {
        by_text: std::collections::BTreeMap<String, Vec<memory_mcp::models::EntityCandidate>>,
    }

    #[async_trait::async_trait]
    impl EntityExtractor for FakeExtractor {
        fn provider_name(&self) -> &'static str {
            "fake"
        }
        async fn extract_candidates(
            &self,
            _content: &str,
        ) -> Result<Vec<memory_mcp::models::EntityCandidate>, memory_mcp::MemoryError> {
            Ok(Vec::new())
        }
        async fn extract_candidates_with_labels(
            &self,
            content: &str,
            _labels: &[String],
        ) -> Result<Vec<memory_mcp::models::EntityCandidate>, memory_mcp::MemoryError> {
            Ok(self.by_text.get(content).cloned().unwrap_or_default())
        }
    }

    fn candidate(name: &str, entity_type: &str) -> memory_mcp::models::EntityCandidate {
        memory_mcp::models::EntityCandidate {
            entity_type: entity_type.to_string(),
            canonical_name: name.to_string(),
            aliases: Vec::new(),
        }
    }

    fn sample_cases() -> Vec<NerQualityCase> {
        vec![
            NerQualityCase {
                id: "q-en-1".into(),
                language: "en".into(),
                text: "Alice Smith from OpenAI presented the Surface Laptop 6 at Build 2026 in Seattle.".into(),
                labels: vec![
                    "person".into(),
                    "company".into(),
                    "product".into(),
                    "event".into(),
                    "location".into(),
                ],
                entities: vec![
                    NerQualityEntity { start: 0, end: 11, text: "Alice Smith".into(), label: "person".into() },
                    NerQualityEntity { start: 17, end: 23, text: "OpenAI".into(), label: "company".into() },
                    NerQualityEntity { start: 72, end: 79, text: "Seattle".into(), label: "location".into() },
                ],
            },
            NerQualityCase {
                id: "q-en-2".into(),
                language: "en".into(),
                text: "At Cloud Summit 2026 in Berlin, Bob Jones and DeepMind compared Pixel 8 Pro with PostgreSQL.".into(),
                labels: vec![
                    "person".into(),
                    "company".into(),
                    "product".into(),
                    "event".into(),
                    "location".into(),
                    "technology".into(),
                ],
                entities: vec![
                    NerQualityEntity { start: 3, end: 20, text: "Cloud Summit 2026".into(), label: "event".into() },
                    NerQualityEntity { start: 32, end: 41, text: "Bob Jones".into(), label: "person".into() },
                ],
            },
        ]
    }

    fn perfect_extractor() -> FakeExtractor {
        let cases = sample_cases();
        FakeExtractor {
            by_text: std::collections::BTreeMap::from([
                (
                    cases[0].text.clone(),
                    vec![
                        candidate("Alice Smith", "person"),
                        candidate("OpenAI", "company"),
                        candidate("Seattle", "location"),
                    ],
                ),
                (
                    cases[1].text.clone(),
                    vec![
                        candidate("Cloud Summit 2026", "event"),
                        candidate("Bob Jones", "person"),
                    ],
                ),
            ]),
        }
    }

    #[tokio::test]
    async fn perfect_extractor_passes_all_cases() {
        let cases = sample_cases();
        let extractor = perfect_extractor();
        for case in &cases {
            let outcome = run_case("ner-quality-fake", &extractor, case).await;
            assert_eq!(outcome.status, CaseStatus::Passed, "case {}", case.id);
            let f1 = outcome.metrics["entity_mention_f1"];
            assert!((f1 - 1.0).abs() < 1e-9, "case {}: f1 = {f1}", case.id);
        }
    }

    #[tokio::test]
    async fn extra_and_missing_mentions_are_scored() {
        let extractor = FakeExtractor {
            by_text: {
                let cases = sample_cases();
                std::collections::BTreeMap::from([(
                    cases[0].text.clone(),
                    vec![
                        candidate("Alice Smith", "person"),
                        candidate("OpenAI", "company"),
                        candidate("NotACorpusEntity", "company"), // FP
                        // Seattle missing -> FN
                    ],
                )])
            },
        };
        let case = sample_cases().into_iter().next().unwrap();
        let outcome = run_case("ner-quality-fake", &extractor, &case).await;
        assert_eq!(outcome.status, CaseStatus::QualityFailed);
        // Case-level: tp=2, fp=1, fn=1 -> precision 2/3, recall 2/3, f1 2/3.
        assert!((outcome.metrics["entity_mention_precision"] - 2.0 / 3.0).abs() < 1e-9);
        assert!((outcome.metrics["entity_mention_recall"] - 2.0 / 3.0).abs() < 1e-9);
        assert!((outcome.metrics["entity_mention_f1"] - 2.0 / 3.0).abs() < 1e-9);
    }

    #[tokio::test]
    async fn typed_diagnostic_punishes_label_mismatch() {
        let extractor = FakeExtractor {
            by_text: {
                let cases = sample_cases();
                std::collections::BTreeMap::from([(
                    cases[0].text.clone(),
                    vec![
                        candidate("Alice Smith", "company"), // name ok, label wrong
                        candidate("OpenAI", "company"),
                        candidate("Seattle", "location"),
                    ],
                )])
            },
        };
        let case = sample_cases().into_iter().next().unwrap();
        let outcome = run_case("ner-quality-fake", &extractor, &case).await;
        assert_eq!(outcome.status, CaseStatus::Passed, "mention match is perfect");
        assert!(
            outcome.metrics["ner_typed_f1"] < 1.0,
            "typed diagnostic must reflect the label mismatch"
        );
    }
}
```

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p eval-harness ner_quality`
Expected: FAIL — module not found / functions undefined.

- [ ] **Step 3: Implement the suite**

Create `crates/eval-harness/src/suites/ner_quality.rs`:

```rust
//! Per-extractor NER quality evaluation.
//!
//! One suite instance per `NER_EXTRACTOR` backend. All instances score the
//! same corpus (`evals/corpora/ner/ner_quality.json`) so the suite summaries
//! render a comparable precision/recall/F1 matrix. Mention matching is
//! case-insensitive on canonical names; typed match is a per-case diagnostic
//! because lightweight backends (regex, anno) use their own type vocabularies.

use std::collections::BTreeSet;
use std::path::PathBuf;
use std::sync::Arc;

use async_trait::async_trait;
use memory_mcp::config::NerExtractorKind;
use memory_mcp::models::EntityCandidate;
use memory_mcp::service::EntityExtractor;
use serde::Deserialize;

use crate::domain::*;
use crate::error::EvalError;
use crate::ner_fixtures;
use crate::reducer::{ClassificationReducer, CountReducer, SuiteReducer};
use crate::runner::{EvalSuite, RunContext};

#[derive(Debug, Clone, Deserialize)]
pub struct NerQualityCase {
    pub id: String,
    pub language: String,
    pub text: String,
    pub labels: Vec<String>,
    pub entities: Vec<NerQualityEntity>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct NerQualityEntity {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub label: String,
}

#[derive(Deserialize)]
struct CorpusFile {
    #[allow(dead_code)]
    schema_version: u32,
    #[allow(dead_code)]
    fixture_status: String,
    #[allow(dead_code)]
    languages: Vec<String>,
    cases: Vec<NerQualityCase>,
}

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/corpora/ner/ner_quality.json")
}

fn load_cases() -> Result<Vec<NerQualityCase>, EvalError> {
    let path = corpus_path();
    let raw = std::fs::read_to_string(&path).map_err(|source| EvalError::Io { path, source })?;
    let corpus: CorpusFile = serde_json::from_str(&raw).map_err(EvalError::Artifact)?;
    Ok(corpus.cases)
}

/// Maps `ner-quality-*` suite ids to their extractor kind.
fn kind_for_id(suite_id: &str) -> Option<NerExtractorKind> {
    match suite_id {
        "ner-quality-anno" => Some(NerExtractorKind::Anno),
        "ner-quality-regex" => Some(NerExtractorKind::Regex),
        "ner-quality-anno-onnx" => Some(NerExtractorKind::AnnoOnnx),
        "ner-quality-gliner" => Some(NerExtractorKind::ClassicGliner),
        "ner-quality-vago" => Some(NerExtractorKind::SauerkrautLfm25),
        _ => None,
    }
}

/// Pushes the `NerQualitySuite` for `suite_id` (must be a `ner-quality-*` id).
pub fn register(suite_id: &str, suites: &mut Vec<Box<dyn EvalSuite>>) -> Result<(), EvalError> {
    let Some(kind) = kind_for_id(suite_id) else {
        return Ok(());
    };
    let suite = NerQualitySuite::new(suite_id.to_string(), kind)?;
    suites.push(Box::new(suite));
    Ok(())
}

/// The reducer depends on fixture availability: when the checkpoint is absent
/// the run emits only `Invalid` outcomes and `ClassificationReducer` would
/// hard-error on zero predictions, so a count-based reducer is used instead
/// (the report then shows the explicit invalid cases).
enum NerSuiteReducer {
    Class(ClassificationReducer),
    Count(CountReducer),
}

pub struct NerQualitySuite {
    id: String,
    kind: NerExtractorKind,
    cases: Vec<NerQualityCase>,
    expected_ids: Vec<EvalCaseId>,
    reducer: NerSuiteReducer,
}

impl NerQualitySuite {
    fn new(id: String, kind: NerExtractorKind) -> Result<Self, EvalError> {
        let cases = load_cases()?;
        let expected_ids = cases
            .iter()
            .map(|c| EvalCaseId::parse(&c.id))
            .collect::<Result<Vec<_>, _>>()?;
        let reducer = if ner_fixtures::fixture_present(kind) {
            NerSuiteReducer::Class(ClassificationReducer::new(id.clone(), "entity_mention"))
        } else {
            NerSuiteReducer::Count(CountReducer::new(id.clone()))
        };
        Ok(Self {
            id,
            kind,
            cases,
            expected_ids,
            reducer,
        })
    }
}

#[async_trait]
impl EvalSuite for NerQualitySuite {
    fn id(&self) -> &str {
        &self.id
    }

    fn mode(&self) -> EvalMode {
        EvalMode::Performance
    }

    fn expected_case_ids(&self) -> &[EvalCaseId] {
        &self.expected_ids
    }

    fn reducer(&self) -> &dyn SuiteReducer {
        match &self.reducer {
            NerSuiteReducer::Class(reducer) => reducer,
            NerSuiteReducer::Count(reducer) => reducer,
        }
    }

    async fn run(&self, _context: &RunContext) -> Vec<EvalCaseOutcome> {
        let fixture_present = ner_fixtures::fixture_present(self.kind);
        if !fixture_present {
            return self
                .cases
                .iter()
                .map(|case| invalid_outcome(&self.id, case, "fixture missing"))
                .collect();
        }
        let Some(extractor) = ner_fixtures::build_extractor(self.kind).await else {
            return self
                .cases
                .iter()
                .map(|case| invalid_outcome(&self.id, case, "fixture missing"))
                .collect();
        };
        let mut outcomes = Vec::with_capacity(self.cases.len());
        for case in &self.cases {
            outcomes.push(run_case(&self.id, extractor.as_ref(), case).await);
        }
        outcomes
    }
}

fn invalid_outcome(suite_id: &str, case: &NerQualityCase, reason: &str) -> EvalCaseOutcome {
    EvalCaseOutcome {
        case_key: CaseKey::parse(suite_id, case.id.as_str()).expect("valid case key"),
        mode: EvalMode::Performance,
        split: CorpusSplit::Test,
        label_trust: LabelTrust::Official,
        status: CaseStatus::Invalid,
        metrics: std::collections::BTreeMap::new(),
        evidence: std::collections::BTreeMap::new(),
        invalid_reason: Some(reason.to_string()),
        failures: vec![],
        duration_ms: 0,
        attempts: 1,
    }
}

pub async fn run_case(
    suite_id: &str,
    extractor: &dyn EntityExtractor,
    case: &NerQualityCase,
) -> EvalCaseOutcome {
    let start = std::time::Instant::now();

    let expected_names: BTreeSet<String> = case
        .entities
        .iter()
        .map(|e| e.text.to_lowercase())
        .collect();

    let predicted = match extractor
        .extract_candidates_with_labels(&case.text, &case.labels)
        .await
    {
        Ok(candidates) => candidates,
        Err(err) => {
            let mut outcome =
                invalid_outcome(suite_id, case, &format!("extraction failed: {err}"));
            outcome.duration_ms = start.elapsed().as_millis() as u64;
            return outcome;
        }
    };

    let predicted_names: BTreeSet<String> = predicted
        .iter()
        .map(|c| c.canonical_name.to_lowercase())
        .collect();

    let tp = expected_names.intersection(&predicted_names).count() as u64;
    let fp = (predicted_names.len() as u64).saturating_sub(tp);
    let fn_ = (expected_names.len() as u64).saturating_sub(tp);

    let evidence = MetricEvidence::classification(tp, fp, fn_, 0);
    let mut metrics = crate::metrics::render_case_metrics(
        &evidence,
        &crate::metrics::CaseMetricNames::classification("entity_mention"),
    );

    // Suite-local diagnostic (not gate-consumed): typed recall-ish score over
    // the expected set, so users see where backends name-match but mislabel.
    let typed_tp = case
        .entities
        .iter()
        .filter(|expected| {
            predicted.iter().any(|candidate| {
                candidate.canonical_name.to_lowercase() == expected.text.to_lowercase()
                    && candidate.entity_type.to_lowercase() == expected.label
            })
        })
        .count() as u64;
    let typed_precision = if predicted.is_empty() {
        0.0
    } else {
        typed_tp as f64 / predicted.len() as f64
    };
    let typed_recall = if expected_names.is_empty() {
        1.0
    } else {
        typed_tp as f64 / expected_names.len() as f64
    };
    let typed_f1 = if typed_precision + typed_recall == 0.0 {
        0.0
    } else {
        2.0 * typed_precision * typed_recall / (typed_precision + typed_recall)
    };
    metrics.insert("ner_typed_f1".into(), typed_f1);

    let mut failures = Vec::new();
    for expected in &case.entities {
        if !predicted_names.contains(&expected.text.to_lowercase()) {
            failures.push(format!("missing mention `{}`", expected.text));
        }
    }
    for candidate in &predicted {
        if !expected_names.contains(&candidate.canonical_name.to_lowercase()) {
            failures.push(format!("unexpected mention `{}`", candidate.canonical_name));
        }
    }

    let case_passed = tp == expected_names.len() as u64 && fp == 0;
    let mut evidence_map = std::collections::BTreeMap::new();
    evidence_map.insert("classification".to_string(), evidence);

    EvalCaseOutcome {
        case_key: CaseKey::parse(suite_id, case.id.as_str()).expect("valid case key"),
        mode: EvalMode::Performance,
        split: CorpusSplit::Test,
        label_trust: LabelTrust::Official,
        status: if case_passed {
            CaseStatus::Passed
        } else {
            CaseStatus::QualityFailed
        },
        metrics,
        evidence: evidence_map,
        invalid_reason: None,
        failures,
        duration_ms: start.elapsed().as_millis() as u64,
        attempts: 1,
    }
}

// (tests from Step 1 live here)
```

- [ ] **Step 4: Update `suites.rs`**

Add to `crates/eval-harness/src/suites.rs` module list (alphabetical, after `lifecycle`):

```rust
pub mod ner_quality;
```

Add re-export after `lifecycle`:

```rust
pub use ner_quality::NerQualitySuite;
```

Add `"ner_quality.rs"` to the `SUITE_FILES` array in the ADR-0025 guard test (alphabetical, after `lifecycle.rs`).

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p eval-harness ner_quality`
Expected: PASS (perfect extractor passes; FP/FN scoring; typed diagnostic; ADR-0025 guard still green).

- [ ] **Step 6: Commit**

```bash
git add crates/eval-harness/src/suites/ner_quality.rs crates/eval-harness/src/suites.rs
git commit -m "feat(eval): add per-extractor NER quality suite"
```

---

### Task 5: `ner_quality` profile + `main.rs` wiring

**Files:**
- Create: `evals/profiles/ner_quality.json`
- Modify: `crates/eval-harness/src/main.rs:4-9, 124-141`

**Interfaces:**
- Consumes: `NerQualitySuite::register` from Task 4, `EvalProfile::NerQuality` from Task 1.
- Produces: profile id `ner_quality`; suite ids `ner-quality-anno|regex|anno-onnx|gliner|vago`; expected coverage per suite = 10.

- [ ] **Step 1: Write the failing profile-load test (extend existing)**

In `crates/eval-harness/src/profile.rs` tests, add:

```rust
#[test]
fn ner_quality_profile_loads() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/profiles/ner_quality.json");
    let manifest = ProfileManifest::load(&path).expect("ner_quality profile must load");
    assert_eq!(
        manifest.suites.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
        vec![
            "ner-quality-anno",
            "ner-quality-regex",
            "ner-quality-anno-onnx",
            "ner-quality-gliner",
            "ner-quality-vago"
        ]
    );
}
```

- [ ] **Step 2: Run test to verify it fails**

Run: `cargo test -p eval-harness ner_quality_profile_loads`
Expected: FAIL — profile file missing.

- [ ] **Step 3: Create the profile**

Create `evals/profiles/ner_quality.json`:

```json
{
  "schema_version": "memory-mcp-eval-profile/v1",
  "profile": "ner_quality",
  "time_budget_seconds": 1800,
  "suites": [
    { "id": "ner-quality-anno", "expected_coverage": { "exact_cases": 10 } },
    { "id": "ner-quality-regex", "expected_coverage": { "exact_cases": 10 } },
    { "id": "ner-quality-anno-onnx", "expected_coverage": { "exact_cases": 10 } },
    { "id": "ner-quality-gliner", "expected_coverage": { "exact_cases": 10 } },
    { "id": "ner-quality-vago", "expected_coverage": { "exact_cases": 10 } }
  ],
  "gates": []
}
```

- [ ] **Step 4: Wire the suites in `main.rs`**

Add `NerQualitySuite` to the import list in `crates/eval-harness/src/main.rs`:

```rust
use eval_harness::{
    ActionGroundingSuite, CapacitySuite, ClaimReconciliationSuite, CorpusManifest, DatasetKind,
    DownstreamQaSuite, EndToEndSuite, ExternalRetrievalSuite, ExtractionSuite,
    LifecycleReleaseSuite, NerQualitySuite, PoisoningSuite, ProfileManifest, ResponseSizeSuite,
    RetrievalSuite, RunArtifact, RunRequest, Runner, SuiteId,
};
```

Add match arms in `cmd_run` (before the `other =>` arm):

```rust
"ner-quality-anno" | "ner-quality-regex" | "ner-quality-anno-onnx"
| "ner-quality-gliner" | "ner-quality-vago" => {
    if let Err(e) = NerQualitySuite::register(&suite_decl.id, &mut suites) {
        eprintln!("warning: failed to load {}: {e}", suite_decl.id);
        issues.push(eval_harness::RunIssue::empty_suite(&suite_decl.id));
    }
}
```

- [ ] **Step 5: Run tests to verify they pass**

Run: `cargo test -p eval-harness ner_quality_profile_loads && cargo build -p eval-harness --bin memory-eval`
Expected: PASS; binary builds.

- [ ] **Step 6: Smoke-run the offline suites**

Run:

```bash
cargo run -p eval-harness --bin memory-eval -- run \
  --profile evals/profiles/ner_quality.json \
  --artifact /tmp/eval-ner-anno.json \
  --suite ner-quality-anno --suite ner-quality-regex
```

Expected: RESULT: PASSED; report shows `entity_mention_f1` per suite (anno and regex rows).

- [ ] **Step 7: Commit**

```bash
git add evals/profiles/ner_quality.json crates/eval-harness/src/main.rs crates/eval-harness/src/profile.rs
git commit -m "feat(eval): add manual NER quality comparison profile"
```

---

### Task 6: All-extractor latency benches

**Files:**
- Modify: `crates/eval-harness/benches/ner_cpu.rs`

**Interfaces:**
- Consumes: `ner_fixtures::{build_extractor, default_labels}` from Task 3.
- Produces: criterion bench names `regex_single_window_warm`, `regex_multi_window_warm`, `anno_single_window_warm`, `anno_multi_window_warm`, `anno_onnx_single_window_warm`, `anno_onnx_multi_window_warm` (plus existing gliner/vago names unchanged); cold-start note printed per extractor.

- [ ] **Step 1: Rewrite the bench file**

Replace the body of `crates/eval-harness/benches/ner_cpu.rs` with:

```rust
//! CPU NER latency benchmarks for every `NER_EXTRACTOR` backend.
//!
//! Model-backed backends are fixture-gated: when the local checkpoint is
//! absent the bench skips with a note so the file still compiles and runs
//! everywhere. `default_service_probe` measures the Anno + DB path and is
//! intentionally kept separate — do not compare across it.

use criterion::{Criterion, black_box, criterion_group, criterion_main};
use eval_harness::ner_fixtures;
use memory_mcp::config::NerExtractorKind;
use memory_mcp::service::capabilities::extract::ExtractCapability;
use memory_mcp::service::EntityExtractor;
use std::sync::Arc;
use std::time::Instant;

fn bench_extractor(
    c: &mut Criterion,
    label: &str,
    kind: NerExtractorKind,
) {
    // Build the extractor once, before Criterion starts timing: the reported
    // "cold start" measures build/first-load time, and each bench then
    // measures the warm steady-state on the already-loaded model.
    let build_start = Instant::now();
    let Some(extractor): Option<Arc<dyn EntityExtractor>> = (move || {
        let rt = tokio::runtime::Runtime::new().unwrap();
        rt.block_on(ner_fixtures::build_extractor(kind))
    })() else {
        eprintln!("{label} benches skipped: local fixture missing");
        return;
    };
    eprintln!("{label} cold start: {:?}", build_start.elapsed());

    let fixture = eval_harness::benchmark::NerBenchmarkFixture::load().unwrap();
    let single = fixture.single_window.to_string();
    let multi = fixture.multi_window.to_string();

    c.bench_function(&format!("{label}_single_window_warm"), |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            let rt = tokio::runtime::Runtime::new().unwrap();
            for _ in 0..iters {
                rt.block_on(async {
                    black_box(
                        extractor
                            .extract_candidates(black_box(&single))
                            .await
                            .unwrap(),
                    );
                });
            }
            start.elapsed()
        })
    });

    c.bench_function(&format!("{label}_multi_window_warm"), |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            let rt = tokio::runtime::Runtime::new().unwrap();
            for _ in 0..iters {
                rt.block_on(async {
                    black_box(
                        extractor
                            .extract_candidates(black_box(&multi))
                            .await
                            .unwrap(),
                    );
                });
            }
            start.elapsed()
        })
    });
}

fn bench_regex(c: &mut Criterion) {
    bench_extractor(c, "regex", NerExtractorKind::Regex);
}

fn bench_anno(c: &mut Criterion) {
    bench_extractor(c, "anno", NerExtractorKind::Anno);
}

fn bench_anno_onnx(c: &mut Criterion) {
    bench_extractor(c, "anno_onnx", NerExtractorKind::AnnoOnnx);
}

fn bench_gliner(c: &mut Criterion) {
    bench_extractor(c, "gliner", NerExtractorKind::ClassicGliner);
}

fn bench_vago(c: &mut Criterion) {
    bench_extractor(c, "vago", NerExtractorKind::SauerkrautLfm25);
}

/// Default-service path probe: measures Anno + DB overhead through the
/// production service path. Kept separate from the per-extractor benches —
/// do not compare across them.
fn bench_default_service_probe(c: &mut Criterion) {
    let rt = tokio::runtime::Runtime::new().unwrap();
    let (service, episode_id) = rt.block_on(async {
        let service = eval_harness::test_support::make_service().await;
        let episode_id = memory_mcp::service::capabilities::ingest::IngestCapability::ingest(
            &service.build_context(),
            memory_mcp::models::IngestRequest {
                source_type: "bench".into(),
                source_id: "probe-001".into(),
                content: "Alice Smith from Acme Corp presented quarterly revenue.".into(),
                t_ref: chrono::Utc::now(),
                scope: "org".into(),
                project: None,
                t_ingested: None,
                visibility_scope: None,
                policy_tags: vec![],
            },
            None,
        )
        .await
        .unwrap();
        (service, episode_id)
    });

    c.bench_function("default_service_extract_warm", |b| {
        b.iter_custom(|iters| {
            let start = Instant::now();
            let rt = tokio::runtime::Runtime::new().unwrap();
            for _ in 0..iters {
                rt.block_on(async {
                    black_box(
                        ExtractCapability::extract(
                            &service.build_context(),
                            &episode_id,
                            None,
                            None,
                        )
                        .await
                        .unwrap(),
                    );
                });
            }
            start.elapsed()
        })
    });
}

criterion_group!(
    benches,
    bench_regex,
    bench_anno,
    bench_anno_onnx,
    bench_gliner,
    bench_vago,
    bench_default_service_probe
);
criterion_main!(benches);
```

- [ ] **Step 2: Verify it compiles**

Run: `cargo check -p eval-harness --benches`
Expected: no errors.

- [ ] **Step 3: Run the offline benches (anno + regex)**

Run: `cargo bench -p eval-harness --bench ner_cpu -- --noplot`
Expected: `regex_*` and `anno_*` benches run with cold-start notes; model benches print "skipped: local fixture missing".

- [ ] **Step 4: Commit**

```bash
git add crates/eval-harness/benches/ner_cpu.rs
git commit -m "feat(eval): benchmark every NER extractor on CPU"
```

---

### Task 7: Fixture-gated real-model quality integration test

**Files:**
- Create: `crates/eval-harness/tests/ner_quality_real_models.rs`

**Interfaces:**
- Consumes: `ner_fixtures::build_extractor`, corpus JSON.
- Produces: `#[ignore]`d integration test proving the real GLiNER checkpoint scores the corpus through `NerQualitySuite`-equivalent logic; runnable locally with `--ignored`.

- [ ] **Step 1: Write the test**

Create `crates/eval-harness/tests/ner_quality_real_models.rs`:

```rust
//! Fixture-gated end-to-end check: the real classic GLiNER checkpoint must
//! build through `ner_fixtures` and score the shared quality corpus without
//! error. Requires the local checkpoint under
//! `crates/memory-mcp/tests/models/ner/urchade--gliner_multi-v2.1/` (gitignored).
//! Run with `--ignored`.

use eval_harness::ner_fixtures;
use eval_harness::suites::ner_quality::{NerQualityCase, run_case};
use memory_mcp::config::NerExtractorKind;

fn load_cases() -> Vec<NerQualityCase> {
    let path = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../evals/corpora/ner/ner_quality.json");
    let raw = std::fs::read_to_string(&path).expect("read corpus");
    #[derive(serde::Deserialize)]
    struct Corpus {
        cases: Vec<NerQualityCase>,
    }
    serde_json::from_str::<Corpus>(&raw).expect("parse corpus").cases
}

#[tokio::test]
#[ignore = "requires the local GLiNER checkpoint under crates/memory-mcp/tests/models/ner/urchade--gliner_multi-v2.1/"]
async fn real_gliner_scores_quality_corpus() {
    // Attribute order is meaningful: `#[tokio::test]` must wrap the async test
    // before `#[ignore]`, so tokio sees the ignore flag on the harnessed body.
    let Some(extractor) = ner_fixtures::build_extractor(NerExtractorKind::ClassicGliner).await
    else {
        panic!("GLiNER fixture missing; run with the checkpoint in place");
    };
    let cases = load_cases();
    assert_eq!(cases.len(), 10);
    for case in &cases {
        let outcome = run_case("ner-quality-gliner", extractor.as_ref(), case).await;
        assert!(
            outcome.status == eval_harness::CaseStatus::Passed
                || outcome.status == eval_harness::CaseStatus::QualityFailed,
            "case {} must produce a scored outcome, got {:?}",
            case.id,
            outcome.status
        );
        assert!(
            outcome.metrics.contains_key("entity_mention_f1"),
            "case {} must carry entity_mention_f1",
            case.id
        );
    }
}
```

**Public surface note:** `NerQualityCase`, `NerQualityEntity`, and `run_case` are `pub` in `crates/eval-harness/src/suites/ner_quality.rs` (Task 4), which is why the integration test can drive the same scoring logic directly with the real extractor.

- [ ] **Step 2: Verify it compiles without the fixture**

Run: `cargo check -p eval-harness --tests`
Expected: compiles (test is `#[ignore]`d and fixture-independent at compile time).

- [ ] **Step 3: Run with the fixture present (local machine)**

Run: `cargo test -p eval-harness --test ner_quality_real_models -- --ignored`
Expected: PASS when the GLiNER checkpoint exists locally.

- [ ] **Step 4: Commit**

```bash
git add crates/eval-harness/tests/ner_quality_real_models.rs crates/eval-harness/src/suites/ner_quality.rs
git commit -m "test(eval): fixture-gated real GLiNER quality run against the corpus"
```

---

### Task 8: Documentation

**Files:**
- Modify: `docs/agent/EVALUATION.md`
- Create: `docs/adr/0037-ner-eval-comparison.md`

- [ ] **Step 1: Add the comparison section to `docs/agent/EVALUATION.md`**

Append after the "Performance Benchmarks" section:

````markdown
## NER Extractor Comparison

Every `NER_EXTRACTOR` backend (`anno`, `regex`, `anno-onnx`, `urchade/gliner_multi-v2.1`,
`VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER`) can be evaluated on the same RU/EN/mixed
quality corpus and latency bench, so you can pick the extractor that fits your scenario.

### Quality (per-extractor mention F1)

```bash
# All five extractors (needs the local model checkpoints under
# crates/memory-mcp/tests/models/ner/, see the README for where to get them):
cargo run -p eval-harness --bin memory-eval -- run \
  --profile evals/profiles/ner_quality.json \
  --artifact target/eval-ner.json

# Only the offline extractors (no checkpoints needed):
cargo run -p eval-harness --bin memory-eval -- run \
  --profile evals/profiles/ner_quality.json \
  --artifact target/eval-ner.json \
  --suite ner-quality-anno --suite ner-quality-regex

# Only the extractors whose checkpoints you have, e.g. GLiNER:
cargo run -p eval-harness --bin memory-eval -- run \
  --profile evals/profiles/ner_quality.json \
  --artifact target/eval-ner.json --suite ner-quality-gliner
```

The markdown report renders one suite summary per extractor:
`entity_mention_precision`, `entity_mention_recall`, `entity_mention_f1` (mention matching is
case-insensitive on canonical names, so type-vocabulary differences between backends
do not distort the comparison). Per-case diagnostics include `ner_typed_f1` and a list
of missing/unexpected mentions. Selecting a suite whose checkpoint is missing produces
explicit `invalid` cases — filter with `--suite` to what you have.

### Performance (latency + cold start)

```bash
cargo bench -p eval-harness --bench ner_cpu -- --noplot
```

Criterion reports `{regex,anno,anno_onnx,gliner,vago}_single_window_warm` and
`_multi_window_warm`; each bench prints the extractor's cold-start time (model load)
before measuring. Model-backed benches skip with a note when the checkpoint is absent.
`default_service_extract_warm` measures the production `ServiceContext::extract` path
(Anno extractor + DB round trip) and is not comparable with the raw-extractor benches.

> **Run one extractor at a time** — the five backends can be very different in
> latency, so compare across suites inside a dedicated run: `cargo run -p
> eval-harness -- run --profile evals/profiles/ner_quality.json ...` (the `--suite`
> flag is repeatable, so you can pass it multiple times).

## Checkpoints

The model-backed suites read **local, gitignored** checkpoints only — nothing is
downloaded. Prepare them by placing the folders under
`crates/memory-mcp/tests/models/ner/`:

| Suite | Fixture dir | How to populate it (all offline after first download) |
|---|---|---|
| `ner-quality-anno-onnx` | `crates/memory-mcp/tests/models/ner/deepanwa--NuNerZero_onnx/` | Download HF `deepanwa/NuNerZero_onnx` (`model.onnx`, `tokenizer.json`, `config.json`) into this dir. |
| `ner-quality-gliner` | `crates/memory-mcp/tests/models/ner/urchade--gliner_multi-v2.1/` | Download HF `urchade/gliner_multi-v2.1` (`model.safetensors`, `gliner_config.json`, `tokenizer.json`) into this dir. |
| `ner-quality-vago` | `crates/memory-mcp/tests/models/ner/VAGOsolutions--SauerkrautLM-LFM2.5-GLiNER/` | Download HF `VAGOsolutions/SauerkrautLM-LFM2.5-GLiNER` (`pytorch_model.bin`, `gliner_config.json`, `tokenizer.json`, ~1.6 GB) into this dir. |

### Interpreting the results

- **regex / anno**: near-instant, zero-download, deterministic. Best for offline-first,
  privacy-sensitive, or high-throughput ingestion where recall of noisy mentions is
  acceptable.
- **anno-onnx**: CPU NuNER, one model download, typed labels. Middle ground for
  single-language precision without a GPU.
- **classic GLiNER**: best general-purpose quality/coverage across RU/EN; largest
  ecosystem default.
- **VAGO LFM2**: strongest RU/EN multilingual zero-shot coverage in a native Candle
  backend; largest checkpoint (~1.6 GB) and longest cold start.
````

- [ ] **Step 2: Create `docs/adr/0037-ner-eval-comparison.md`**

```markdown
# ADR-0037: NER Extractor Evaluation Comparison

> Status: Accepted
> Date: 2026-08-08
> Related: ADR-0025 (suite metric provenance), ADR-0036 (unified NER_EXTRACTOR)

## Context

The closed `NER_EXTRACTOR` catalog spans five backends with very different profiles:
offline rules (anno/regex), CPU ONNX (anno-onnx), and two native Candle GLiNERs
(classic and VAGO LFM2). Users need a way to compare their quality and latency on
the same inputs to choose the right backend for a scenario. The eval harness had
latency benches for GLiNER/VAGO only, and no quality suite parameterized over
extractors.

## Decision

Add a manual-only evaluation workflow, in `eval-harness`, covering every backend:

1. A shared RU/EN/mixed quality corpus (`evals/corpora/ner/ner_quality.json`) with
   hand-annotated spans/labels, structurally validated offline. Six cases reuse the
   VAGO release-parity corpus verbatim (pinned by a consistency test).
2. One `NerQualitySuite` per extractor (`ner-quality-anno|regex|anno-onnx|gliner|vago`)
   that builds the extractor through the production constructors — the store-free
   paths (`create_entity_extractor` with an explicit `cache_dir` for anno-onnx,
   `GlinerEntityExtractor::new` and `VagoLfm2EntityExtractor::new_with_runtime` for
   the model-backed kinds) — fixture-gated (missing or incomplete checkpoint =>
   explicit invalid cases, never a revision resolve or download). Mention-level
   precision/recall/F1 use the existing `ClassificationReducer`;
   typed match is a per-case diagnostic.
3. Full CPU latency bench coverage (`ner_cpu.rs`) with cold-start reporting.
4. A dedicated `ner_quality` profile; the suites are excluded from `pr`/`release`/
   `nightly` profiles because CI has no model checkpoints and any invalid outcome
   invalidates a run.

## Consequences

Users get a comparable quality + latency matrix per extractor. Model-backed suites
require locally prepared checkpoints (gitignored fixtures). Suite ids are stable and
mirror the `NER_EXTRACTOR` selectors. Adding a future backend means: extend the
corpus if needed, add one suite registration, and one bench.
```

- [ ] **Step 3: Verify docs build check**

Run: `git diff --stat` and re-read both files for typos. No code tests apply to docs.

- [ ] **Step 4: Commit**

```bash
git add docs/agent/EVALUATION.md docs/adr/0037-ner-eval-comparison.md
git commit -m "docs(eval): document NER extractor comparison workflow and ADR-0037"
```

---

## Final validation gate

- [ ] `cargo fmt --all --check` — zero diff
- [ ] `cargo clippy --workspace --all-targets --features cli-watch,mcp-apps --locked -- -D warnings` — zero warnings
- [ ] `cargo test --workspace --lib --bins --tests --locked` — all green (new offline tests included; `#[ignore]`d model tests skipped)
- [ ] `cargo check -p eval-harness --benches` — benches compile
- [ ] Smoke: `cargo run -p eval-harness --bin memory-eval -- run --profile evals/profiles/ner_quality.json --artifact /tmp/eval-ner.json --suite ner-quality-anno --suite ner-quality-regex` → the run completes with per-extractor suite summaries (mention-level metrics present; `RESULT: QUALITY FAILED` is expected — the corpus is deliberately hard for heuristics; what matters is that the harness renders, not that extractors pass)
- [ ] Manual (local machine with checkpoints): `cargo test -p eval-harness --test ner_quality_real_models -- --ignored` → PASS
- [ ] Manual: `cargo bench -p eval-harness --bench ner_cpu -- --noplot` → regex/anno run, model benches skip with notes

## Self-Review

**Spec coverage:** Every extractor gets quality (Task 4 + corpus Task 2) and latency (Task 6) coverage; the comparison report/matrix comes from the existing suite-summary rendering (Task 5); choosing guidance is documented (Task 8). ✓

**Placeholder scan:** All code steps carry full implementations; the corpus JSON is complete with verified character offsets; the Task 7 seam note is the only conditional content, and it is runnable code. ✓

**Type consistency:** `ner_fixtures::build_extractor(kind) -> Option<Arc<dyn EntityExtractor>>` is used identically by benches (Task 6) and the suite (Task 4); `NerQualitySuite::register` signature matches the `main.rs` call; metric names `entity_mention_precision/recall/f1` are produced by `ClassificationReducer` with prefix `entity_mention` (same as the existing `extraction.rs` suite) in both `run_case` (via `render_case_metrics`) and the reducer — consistent. `EvalProfile::NerQuality` serializes as `ner_quality`, matching the profile JSON. ✓

**Contradiction check:** The ADR-0025 guard list is extended with `ner_quality.rs`; the suite never literal-inserts `entity_mention_*` (only the diagnostic `ner_typed_f1`, consistent with the `fact_type_accuracy` precedent). Model suites are manual-only (verdict semantics documented). Model-backed kinds build through the production store-free constructors (`GlinerEntityExtractor::new`, `VagoLfm2EntityExtractor::new_with_runtime`, `create_entity_extractor` with explicit `cache_dir` for anno-onnx), so no revision is resolved and nothing is downloaded at eval time. ✓
