mod eval_support;

#[test]
fn retrieval_dataset_rejects_contradictory_expectations() {
    let json = r#"
    [
      {
        "id": "ret-bad-001",
        "description": "contradictory case",
        "episodes": [],
        "query": { "query": "atlas", "scope": "personal", "budget": 5, "as_of": null },
        "expected": {
          "must_contain": ["Atlas"],
          "must_not_contain": [],
          "expect_empty": true,
          "min_recall_at_k": 1.0
        }
      }
    ]
    "#;

    let err = eval_support::dataset::parse_retrieval_cases(json).unwrap_err();
    assert!(err.to_string().contains("expect_empty"));
}
