//! Agent-independent public-MCP history formation for isolated evaluations.
use crate::EvalError;
use crate::paired_agent::{SeedEpisode, SeedMemoryOutput};
use crate::suites::memory_decisions::stdio::StdioSession;
use chrono::{DateTime, Utc};
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::Path;

pub(crate) fn seed_memory(
    memory_server_binary: &Path,
    database_path: &Path,
    claim_rollout_stage: &str,
    episodes: &[SeedEpisode],
    checkpoint: Option<(usize, DateTime<Utc>)>,
) -> Result<SeedMemoryOutput, EvalError> {
    let mut session = StdioSession::spawn_with_claim_stage(
        memory_server_binary,
        database_path,
        claim_rollout_stage,
    )?;
    let schemas = session.tool_schemas().clone();
    let mut bindings = BTreeMap::new();
    if checkpoint.is_some_and(|(index, _)| index > episodes.len()) {
        return Err(EvalError::InvalidInput(
            "paired revision checkpoint lies beyond the seeded history".into(),
        ));
    }
    let mut checkpoint_revision = None;
    for (index, episode) in episodes.iter().enumerate() {
        if let Some((checkpoint_index, valid_at)) = checkpoint
            && index == checkpoint_index
        {
            checkpoint_revision = Some(read_known_revision(&mut session, valid_at)?);
        }
        let ingest = session.call_tool(
            "ingest",
            serde_json::json!({
                "source_type": episode.source_type,
                "source_id": episode.source_id,
                "content": episode.content,
                "t_ref": episode.valid_at.to_rfc3339(),
            }),
        )?;
        let episode_id = ingest
            .pointer("/result/structuredContent/result")
            .and_then(Value::as_str)
            .filter(|id| !id.trim().is_empty())
            .ok_or_else(|| {
                EvalError::InvalidInput("paired trial ingest did not return an episode ID".into())
            })?
            .to_string();
        if bindings
            .insert(episode_id.clone(), episode.event_id.clone())
            .is_some()
        {
            return Err(EvalError::InvalidInput(
                "paired trial ingest returned a duplicate episode ID".into(),
            ));
        }
        session.call_tool("extract", serde_json::json!({"episode_id":episode_id}))?;
    }
    if let Some((checkpoint_index, valid_at)) = checkpoint
        && checkpoint_index == episodes.len()
    {
        checkpoint_revision = Some(read_known_revision(&mut session, valid_at)?);
    }
    session.shutdown()?;
    Ok(SeedMemoryOutput {
        episode_id_to_event_id: bindings,
        tool_schemas: schemas,
        known_revision: checkpoint_revision,
    })
}

fn read_known_revision(
    session: &mut StdioSession,
    valid_at: DateTime<Utc>,
) -> Result<i64, EvalError> {
    let response = session.call_tool(
        "assemble_context",
        serde_json::json!({
            "query":"",
            "valid_at":valid_at.to_rfc3339(),
            "budget":1,
            "compact":true
        }),
    )?;
    response
        .pointer("/result/structuredContent/context/snapshot/known_revision")
        .and_then(Value::as_i64)
        .ok_or_else(|| {
            EvalError::InvalidInput(
                "assemble_context checkpoint response lacks known_revision".into(),
            )
        })
}
