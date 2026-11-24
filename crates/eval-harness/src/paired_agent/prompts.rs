//! Shared, runner-neutral prompt assets and identity fingerprints.

use sha2::{Digest, Sha256};

use super::sha256;

pub(crate) const RECALL_SYSTEM_INSTRUCTIONS: &str =
    include_str!("../../../../evals/paired/prompts/memory-recall-system-v1.txt");
pub(crate) const RECALL_TASK_RULES: &str =
    include_str!("../../../../evals/paired/prompts/memory-recall-task-rules-v2.txt");
pub(crate) const FORMATION_SYSTEM_INSTRUCTIONS: &str =
    include_str!("../../../../evals/paired/prompts/agent-memory-formation-system-v1.txt");
pub(crate) const FORMATION_TASK_RULES: &str =
    include_str!("../../../../evals/paired/prompts/agent-memory-formation-task-rules-v1.txt");

pub(crate) fn source_authority_system_prompt_sha256() -> String {
    hash_prompt_components(
        "memory-mcp-source-authority-system-prompts/v1",
        &[
            FORMATION_SYSTEM_INSTRUCTIONS,
            RECALL_SYSTEM_INSTRUCTIONS,
            RECALL_SYSTEM_INSTRUCTIONS,
        ],
    )
}

pub(crate) fn source_authority_task_prompt_sha256() -> String {
    hash_prompt_components(
        "memory-mcp-source-authority-task-rules/v2",
        &[FORMATION_TASK_RULES, RECALL_TASK_RULES, RECALL_TASK_RULES],
    )
}

pub(crate) fn source_authority_protocol_config_sha256() -> String {
    sha256(include_bytes!(
        "../../../../evals/paired/source-authority-agent-workflow-v1/protocol-config.json"
    ))
}

pub(crate) fn source_authority_simulator_sha256() -> String {
    sha256(include_bytes!(
        "../../../../evals/paired/source-authority-agent-workflow-v1/simulator.json"
    ))
}

pub(crate) fn source_authority_literal_input_protocol_config_sha256() -> String {
    sha256(include_bytes!(
        "../../../../evals/paired/source-authority-agent-workflow-literal-input-v1/protocol-config.json"
    ))
}

pub(crate) fn source_authority_literal_input_simulator_sha256() -> String {
    sha256(include_bytes!(
        "../../../../evals/paired/source-authority-agent-workflow-literal-input-v1/simulator.json"
    ))
}

fn hash_prompt_components(domain: &str, prompts: &[&str]) -> String {
    let mut digest = Sha256::new();
    for component in std::iter::once(domain).chain(prompts.iter().copied()) {
        digest.update((component.len() as u64).to_be_bytes());
        digest.update(component.as_bytes());
    }
    hex::encode(digest.finalize())
}
