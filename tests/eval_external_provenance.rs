mod eval_support;

use std::path::PathBuf;

use eval_support::external::{
    DatasetKind, fixture_provenance, verify_fixture_provenance_against_source,
};

fn longmemeval_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("evals")
        .join("raw")
        .join("longmemeval")
        .join("sample_longmemeval_s_cleaned.json")
}

fn locomo_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("evals")
        .join("raw")
        .join("locomo")
        .join("sample_locomo10.json")
}

fn personamem_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("evals")
        .join("raw")
        .join("personamem")
        .join("sample_personamem_32k.json")
}

fn prefeval_fixture_path() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests")
        .join("fixtures")
        .join("evals")
        .join("raw")
        .join("prefeval")
        .join("sample_travel_hotel_implicit_persona.json")
}

#[test]
fn declares_trimmed_source_metadata_for_external_fixtures() {
    let longmemeval = fixture_provenance(DatasetKind::LongMemEvalCleaned);
    assert_eq!(longmemeval.fixture_kind, "trimmed_official_excerpt");
    assert!(
        longmemeval
            .source_url
            .ends_with("longmemeval_s_cleaned.json"),
        "unexpected longmemeval source url: {}",
        longmemeval.source_url
    );

    let locomo = fixture_provenance(DatasetKind::LoCoMo);
    assert_eq!(locomo.fixture_kind, "trimmed_official_excerpt");
    assert!(
        locomo.source_url.ends_with("data/locomo10.json"),
        "unexpected locomo source url: {}",
        locomo.source_url
    );

    let personamem = fixture_provenance(DatasetKind::PersonaMem);
    assert_eq!(personamem.fixture_kind, "trimmed_official_excerpt");
    assert!(
        personamem.source_url.contains("questions_32k.csv"),
        "unexpected personamem source url: {}",
        personamem.source_url
    );
    assert!(
        personamem
            .auxiliary_source_url
            .is_some_and(|url| url.contains("shared_contexts_32k.jsonl")),
        "unexpected personamem auxiliary source: {:?}",
        personamem.auxiliary_source_url
    );

    let prefeval = fixture_provenance(DatasetKind::PrefEval);
    assert_eq!(prefeval.fixture_kind, "trimmed_official_excerpt");
    assert!(
        prefeval
            .source_url
            .ends_with("travel_hotel_overall300_topk_history_persona.json"),
        "unexpected prefeval source url: {}",
        prefeval.source_url
    );
}

#[tokio::test]
#[ignore]
async fn verify_external_fixtures_against_official_sources() {
    let checks = [
        (DatasetKind::LongMemEvalCleaned, longmemeval_fixture_path()),
        (DatasetKind::LoCoMo, locomo_fixture_path()),
        (DatasetKind::PersonaMem, personamem_fixture_path()),
        (DatasetKind::PrefEval, prefeval_fixture_path()),
    ];

    for (kind, path) in checks {
        println!(
            "verifying fixture provenance for {:?} from {}",
            kind,
            path.display()
        );
        let raw = std::fs::read_to_string(&path)
            .unwrap_or_else(|err| panic!("failed to read fixture {}: {err}", path.display()));
        verify_fixture_provenance_against_source(kind, &raw)
            .await
            .unwrap_or_else(|err| panic!("fixture provenance check failed for {:?}: {err}", kind));
    }
}
