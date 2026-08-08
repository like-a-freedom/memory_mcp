//! Offline structural validation of the shared NER quality corpus.

use std::path::PathBuf;

fn corpus_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../evals/corpora/ner/ner_quality.json")
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
        assert!(
            !case.entities.is_empty(),
            "case {} has no entities",
            case.id
        );
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
        assert_eq!(
            shared.text, parity_case.text,
            "case q-{}: text drift",
            parity_case.id
        );
        assert_eq!(shared.entities.len(), parity_case.entities.len());
        for (ours, theirs) in shared.entities.iter().zip(parity_case.entities.iter()) {
            assert_eq!(
                ours.start, theirs.start,
                "case q-{}: start drift",
                parity_case.id
            );
            assert_eq!(ours.end, theirs.end, "case q-{}: end drift", parity_case.id);
            assert_eq!(
                ours.text, theirs.text,
                "case q-{}: text drift",
                parity_case.id
            );
            assert_eq!(
                ours.label, theirs.label,
                "case q-{}: label drift",
                parity_case.id
            );
        }
    }
}
