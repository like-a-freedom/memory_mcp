use eval_harness::codex_agent::{CodexMemoryToolProfile, task_rules_for_profile};

#[test]
fn source_authority_recall_prompt_requires_a_concise_answer_value() {
    let task_rules = task_rules_for_profile(CodexMemoryToolProfile::RecallOnly);

    assert!(task_rules.contains("shortest direct value that answers the user's question"));
    assert!(task_rules.contains("Do not include a preamble, explanation, or additional facts"));
}
