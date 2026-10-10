use clap::Parser;
use eval_harness::cli::Cli;

#[test]
fn calibration_command_accepts_source_pairs_and_an_output_artifact() {
    let parsed = Cli::try_parse_from([
        "memory-eval",
        "calibrate-reranking",
        "--pairs",
        "reviewed-pairs.json",
        "--output",
        "calibration.json",
    ]);
    assert!(
        parsed.is_ok(),
        "the real CLI must expose the independent calibration operation"
    );
}

#[test]
fn pinned_model_preparation_is_a_real_cli_operation() {
    assert!(
        Cli::try_parse_from([
            "memory-eval",
            "prepare-model",
            "--manifest",
            "model.json",
            "--output-root",
            "cache"
        ])
        .is_ok()
    );
}

#[tokio::test]
#[ignore = "requires independently reviewed pairs and verified native model caches; set operational floor to zero"]
async fn native_calibration_distinguishes_fitted_quality_from_the_applied_boundary() {
    let pairs = std::env::var("NATIVE_CALIBRATION_PAIRS").expect("source-reviewed pairs");
    let report = eval_harness::reranker_calibration::calibrate_native(std::path::Path::new(&pairs))
        .await
        .expect("real calibration");
    let configured = report
        .get("configured_operating_point")
        .expect("report must measure the boundary actually configured in production");
    assert_eq!(configured["floor"], 0.0);
    assert!(
        configured["false_negative"].as_u64().expect("count") > 0,
        "zero rejects independently reviewed negative-logit positives"
    );
    assert_eq!(report["operating_point"]["false_negative"], 0);
}
