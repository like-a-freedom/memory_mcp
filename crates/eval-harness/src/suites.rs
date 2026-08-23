pub mod action_grounding;
pub mod capacity;
pub mod claims;
pub mod end_to_end;
pub mod external_retrieval;
pub mod extraction;
pub mod lifecycle;
pub mod ner_quality;
pub mod poisoning;
pub mod registry;
pub mod response_size;
pub mod retrieval;
pub(crate) mod retrieval_cases;

pub use action_grounding::ActionGroundingSuite;
pub use capacity::CapacitySuite;
pub use claims::ClaimReconciliationSuite;
pub use end_to_end::EndToEndSuite;
pub use external_retrieval::{ExternalRetrievalSuite, WorkerPolicy};
pub use extraction::ExtractionSuite;
pub use lifecycle::LifecycleReleaseSuite;
pub use ner_quality::NerQualitySuite;
pub use poisoning::PoisoningSuite;
pub use response_size::ResponseSizeSuite;
pub use retrieval::LocalRetrievalSuite as RetrievalSuite;

#[cfg(test)]
mod tests {
    //! ADR-0025 guard: no suite module may hand-build a gate-consumed metric
    //! key. Gate-consumed diagnostic values must be produced by
    //! `crate::metrics::render_case_metrics` from typed `MetricEvidence`,
    //! never by a literal `metrics.insert("<gate_key>", ...)` in a suite.

    use std::path::PathBuf;

    /// Gate metric names (and prefixes for cutoff-parameterized names) that
    /// suites must not materialize via string-literal map writes.
    const FORBIDDEN_LITERALS: &[&str] = &[
        "recall_at",
        "mrr",
        "top_1_hit_rate",
        "entity_f1",
        "entity_precision",
        "entity_recall",
        "entity_mention_f1",
        "entity_mention_precision",
        "entity_mention_recall",
        "claim_precision",
        "claim_recall",
    ];

    const SUITE_FILES: &[&str] = &[
        "action_grounding.rs",
        "capacity.rs",
        "claims.rs",
        "end_to_end.rs",
        "external_retrieval.rs",
        "extraction.rs",
        "lifecycle.rs",
        "ner_quality.rs",
        "poisoning.rs",
        "response_size.rs",
        "retrieval.rs",
    ];

    #[test]
    fn no_suite_hard_codes_gate_metric_keys() {
        let suites_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("src")
            .join("suites");
        let mut violations = Vec::new();

        for file in SUITE_FILES {
            let path = suites_dir.join(file);
            let source = std::fs::read_to_string(&path)
                .unwrap_or_else(|err| panic!("cannot read {}: {err}", path.display()));
            for (lineno, line) in source.lines().enumerate() {
                let trimmed = line.trim();
                // Detect `...insert("<forbidden>..."` where the forbidden key
                // appears as a string literal at the start of an insert call.
                if !trimmed.contains(".insert(") {
                    continue;
                }
                for key in FORBIDDEN_LITERALS {
                    // Match `.insert("<key>` — string literal directly in the
                    // insert call. Dynamic format!() keys do not match here
                    // by design: the guard targets literal duplication.
                    let needle = format!(".insert(\"{key}");
                    if trimmed.contains(&needle) {
                        violations.push(format!(
                            "{}:{}: literal gate metric key `{key}` in `{trimmed}`",
                            path.display(),
                            lineno + 1
                        ));
                    }
                }
            }
        }

        assert!(
            violations.is_empty(),
            "ADR-0025 violation: suite modules must not hard-code gate metric keys\n{}",
            violations.join("\n")
        );
    }
}
