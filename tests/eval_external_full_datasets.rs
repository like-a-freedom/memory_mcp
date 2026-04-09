mod eval_support;

use eval_support::external::{DatasetKind, normalize_external_dataset};
use eval_support::external_full::{
    bundle_personamem_official_sources, full_dataset_cache_path, load_external_dataset_cases,
    wrap_prefeval_full_track,
};

#[test]
fn wraps_prefeval_upstream_array_into_track_bundle_for_normalization() {
    let upstream_array = r#"[
        {
            "preference": "I absolutely avoid hotels with a bustling nightlife atmosphere.",
            "question": "Can you suggest some great hotels for my upcoming trip to Las Vegas?",
            "explanation": "Need quieter venues.",
            "model": "gpt4o",
            "violation_probability": 1.0,
            "persona": "A police officer specializing in community outreach programs",
            "conversation": {
                "0": {
                    "user": "I usually prefer quieter hotels away from the city center.",
                    "assistant": "Understood."
                }
            }
        }
    ]"#;

    let wrapped = wrap_prefeval_full_track(
        "travel_hotel_overall300_topk_history_persona",
        upstream_array,
    )
    .expect("wrap full prefeval track");

    let cases = normalize_external_dataset(DatasetKind::PrefEval, &wrapped)
        .expect("normalize wrapped prefeval track");

    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0].dataset, "prefeval");
    assert_eq!(
        cases[0].metadata["track"],
        "travel_hotel_overall300_topk_history_persona"
    );
}

#[test]
fn bundles_personamem_official_sources_into_normalizer_fixture() {
    let questions_csv = concat!(
        "persona_id,question_id,question_type,topic,context_length_in_tokens,context_length_in_letters,distance_to_ref_in_blocks,distance_to_ref_in_tokens,num_irrelevant_tokens,distance_to_ref_proportion_in_context,user_question_or_message,correct_answer,all_options,shared_context_id,end_index_in_shared_context\n",
        "0,question-1,recall_user_shared_facts,musicRecommendation,10,100,1,5,0,50%,\"I recently attended a Pacific fusion concert.\",\"(a)\",\"[\"\"(a) It sounds like the Pacific fusion concert was unforgettable.\"\", \"\"(b) Something unrelated\"\"]\",ctx-1,1\n"
    );
    let shared_contexts_jsonl = "{\"ctx-1\":[{\"role\":\"user\",\"content\":\"User: I recently attended a Pacific fusion concert and loved the modern beats.\"}]}\n";

    let bundled = bundle_personamem_official_sources(questions_csv, shared_contexts_jsonl)
        .expect("bundle personamem official sources");
    let cases = normalize_external_dataset(DatasetKind::PersonaMem, &bundled)
        .expect("normalize bundled personamem sources");

    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0].id, "personamem:question-1");
    assert_eq!(cases[0].facts.len(), 1);
    assert!(cases[0].expected.must_contain[0].contains("Pacific fusion concert"));
}

#[test]
fn bundles_personamem_python_style_options_into_normalizer_fixture() {
    let questions_csv = concat!(
        "persona_id,question_id,question_type,topic,context_length_in_tokens,context_length_in_letters,distance_to_ref_in_blocks,distance_to_ref_in_tokens,num_irrelevant_tokens,distance_to_ref_proportion_in_context,user_question_or_message,correct_answer,all_options,shared_context_id,end_index_in_shared_context\n",
        "0,question-2,recall_user_shared_facts,musicRecommendation,10,100,1,5,0,50%,\"I revisited a Pacific fusion concert.\",\"(c)\",\"['(a) It sounded unforgettable.', '(b) Something unrelated', \"\"(c) It's great to revisit Pacific fusion sounds.\"\", '(d) Another unrelated option']\",ctx-2,1\n"
    );
    let shared_contexts_jsonl = "{\"ctx-2\":[{\"role\":\"user\",\"content\":\"User: I revisited a Pacific fusion concert and loved the modern beats.\"}]}\n";

    let bundled = bundle_personamem_official_sources(questions_csv, shared_contexts_jsonl)
        .expect("bundle personamem official sources with python-style options");
    let cases = normalize_external_dataset(DatasetKind::PersonaMem, &bundled)
        .expect("normalize bundled personamem python-style sources");

    assert_eq!(cases.len(), 1);
    assert_eq!(cases[0].id, "personamem:question-2");
    assert_eq!(
        cases[0].metadata["selected_option"],
        "(c) It's great to revisit Pacific fusion sounds."
    );
}

#[test]
fn full_dataset_cache_path_uses_full_fixture_directory() {
    let path = full_dataset_cache_path(DatasetKind::LoCoMo);
    let normalized = path.to_string_lossy().replace('\\', "/");

    assert!(
        normalized.ends_with("tests/fixtures/evals/full/locomo/locomo10.json"),
        "unexpected full dataset cache path: {normalized}"
    );
}

#[tokio::test]
#[ignore]
async fn loads_full_longmemeval_cases_from_official_source() {
    let cases = load_external_dataset_cases(DatasetKind::LongMemEvalCleaned)
        .await
        .expect("load full longmemeval cases");

    assert!(
        cases.len() >= 100,
        "expected at least 100 longmemeval cases, got {}",
        cases.len()
    );
}

#[tokio::test]
#[ignore]
async fn loads_full_locomo_cases_from_official_source() {
    let cases = load_external_dataset_cases(DatasetKind::LoCoMo)
        .await
        .expect("load full locomo cases");

    assert!(
        cases.len() >= 100,
        "expected at least 100 locomo cases, got {}",
        cases.len()
    );
}

#[tokio::test]
#[ignore]
async fn loads_full_prefeval_cases_from_official_source() {
    let cases = load_external_dataset_cases(DatasetKind::PrefEval)
        .await
        .expect("load full prefeval cases");

    assert!(
        cases.len() >= 50,
        "expected at least 50 prefeval cases, got {}",
        cases.len()
    );
}

#[tokio::test]
#[ignore]
async fn loads_full_personamem_cases_from_official_source() {
    let cases = load_external_dataset_cases(DatasetKind::PersonaMem)
        .await
        .expect("load full personamem cases");

    assert!(
        cases.len() >= 500,
        "expected the official full personamem source to yield hundreds of cases, got {}",
        cases.len()
    );
}
