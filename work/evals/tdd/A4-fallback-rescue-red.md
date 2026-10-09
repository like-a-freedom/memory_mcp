# A4 RED evidence — episode fallback and first-person rescue

These tests ran before the A4 production predicate/guard was added. Both failures were behavioral; compilation and the isolated database setup succeeded.

- rtk cargo test -p memory_mcp --locked --features eval-support --test assemble_context_semantic_quality episode_fallback_rejects_a_single_keyword_match_for_a_multiterm_query — exit 101. The service returned an episode sharing only the word cobalt for a multi-term query; the test expected the one-term distractor to be excluded.
- rtk cargo test -p memory_mcp --locked --lib first_person_fact_rescue_skips_candidate_without_admission_evidence — exit 101. The first-person rescue appended a User: candidate whose CandidateEvidence had no admission basis.

The episode case was later strengthened to seed both episodes through the production ingestion capability and assert that only the multi-term grounded episode survives.
