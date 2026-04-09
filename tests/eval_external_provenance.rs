mod eval_support;

use eval_support::external::{DatasetKind, fixture_provenance, normalize_external_dataset};
use eval_support::external_full::raw_fixture_path;

#[test]
fn declares_full_dataset_metadata_for_external_fixtures() {
    let longmemeval = fixture_provenance(DatasetKind::LongMemEvalCleaned);
    assert_eq!(longmemeval.fixture_kind, "full_official_dataset");
    assert!(
        longmemeval
            .source_url
            .ends_with("longmemeval_s_cleaned.json"),
        "unexpected longmemeval source url: {}",
        longmemeval.source_url
    );

    let locomo = fixture_provenance(DatasetKind::LoCoMo);
    assert_eq!(locomo.fixture_kind, "full_official_dataset");
    assert!(
        locomo.source_url.ends_with("data/locomo10.json"),
        "unexpected locomo source url: {}",
        locomo.source_url
    );

    let personamem = fixture_provenance(DatasetKind::PersonaMem);
    assert_eq!(personamem.fixture_kind, "full_official_dataset");
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
    assert_eq!(prefeval.fixture_kind, "full_official_dataset");
    assert!(
        prefeval
            .source_url
            .ends_with("travel_hotel_overall300_topk_history_persona.json"),
        "unexpected prefeval source url: {}",
        prefeval.source_url
    );
}

#[test]
fn raw_fixture_files_exist_for_all_datasets() {
    let kinds = [
        (
            DatasetKind::LongMemEvalCleaned,
            "longmemeval_s_cleaned.json",
        ),
        (DatasetKind::LoCoMo, "locomo10.json"),
        (DatasetKind::PersonaMem, "questions_32k.csv"),
        (
            DatasetKind::PrefEval,
            "travel_hotel_overall300_topk_history_persona.json",
        ),
    ];

    for (kind, expected_file) in kinds {
        let path = raw_fixture_path(kind);
        assert!(
            path.exists(),
            "raw fixture for {:?} should exist at {} (expected file: {})",
            kind,
            path.display(),
            expected_file,
        );
    }
}

#[test]
fn raw_fixtures_normalize_into_cases() {
    let kinds = [
        DatasetKind::LongMemEvalCleaned,
        DatasetKind::LoCoMo,
        DatasetKind::PersonaMem,
        DatasetKind::PrefEval,
    ];

    for kind in kinds {
        let path = raw_fixture_path(kind);
        let raw = std::fs::read_to_string(&path).unwrap_or_else(|err| {
            panic!(
                "read raw fixture for {:?} at {}: {err}",
                kind,
                path.display()
            )
        });

        // PersonaMem and PrefEval need bundling — skip normalization here
        // since the external_full module handles bundling.
        if matches!(kind, DatasetKind::PersonaMem | DatasetKind::PrefEval) {
            continue;
        }

        let cases = normalize_external_dataset(kind, &raw)
            .unwrap_or_else(|err| panic!("normalize raw fixture for {:?}: {err}", kind));
        assert!(
            !cases.is_empty(),
            "raw fixture for {:?} should normalize into at least 1 case",
            kind
        );
    }
}
