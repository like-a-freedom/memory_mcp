//! Explicit opt-in evidence for public synthetic tasks. Never wrap private data.
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::chat_completions::MemoryToolSession;
use super::sha256;
use crate::EvalError;
use crate::paired::{CaseTrialKey, RunArm};

const SCHEMA: &str = "memory-mcp-public-synthetic-trace/v1";

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SyntheticTraceIdentity {
    pub plan_sha256: String,
    pub product_binary_sha256: String,
    pub tool_schema_sha256: String,
    pub key: CaseTrialKey,
    pub arm: RunArm,
    pub phase: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PublicSyntheticTrace {
    schema_version: String,
    pub identity: SyntheticTraceIdentity,
    calls: Vec<PublicCall>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublicCall {
    tool: String,
    arguments: Value,
    arguments_sha256: String,
    response: Option<Value>,
    response_sha256: Option<String>,
    /// True only when the transport returned no response; inspect raw envelopes for MCP errors.
    call_failed: bool,
}

pub struct PublicSyntheticSession<'a, M> {
    memory: &'a mut M,
    trace: PublicSyntheticTrace,
}

impl<'a, M: MemoryToolSession> PublicSyntheticSession<'a, M> {
    pub fn new(memory: &'a mut M, identity: SyntheticTraceIdentity) -> Result<Self, EvalError> {
        let trace = PublicSyntheticTrace {
            schema_version: SCHEMA.into(),
            identity,
            calls: vec![],
        };
        trace.validate()?;
        trace.validate_schema(memory)?;
        Ok(Self { memory, trace })
    }

    pub fn finish(self) -> PublicSyntheticTrace {
        self.trace
    }
}

impl<M: MemoryToolSession> MemoryToolSession for PublicSyntheticSession<'_, M> {
    fn tool_schemas(&self) -> &Value {
        self.memory.tool_schemas()
    }

    fn call_tool(&mut self, name: &str, arguments: Value) -> Result<Value, EvalError> {
        let arguments_sha256 = hash(&arguments)?;
        let result = self.memory.call_tool(name, arguments.clone());
        let response = result.as_ref().ok().cloned();
        let response_sha256 = response.as_ref().map(hash).transpose()?;
        self.trace.calls.push(PublicCall {
            tool: name.into(),
            arguments,
            arguments_sha256,
            response,
            response_sha256,
            call_failed: result.is_err(),
        });
        result
    }
}

impl PublicSyntheticTrace {
    pub fn validate(&self) -> Result<(), EvalError> {
        let identity = &self.identity;
        if self.schema_version != SCHEMA
            || !matches!(identity.phase.as_str(), "formation" | "recall")
            || [
                &identity.plan_sha256,
                &identity.product_binary_sha256,
                &identity.tool_schema_sha256,
            ]
            .iter()
            .any(|digest| digest.len() != 64 || !digest.bytes().all(|b| b.is_ascii_hexdigit()))
            || [
                &identity.key.stratum_id,
                &identity.key.family_id,
                &identity.key.case_id,
            ]
            .iter()
            .any(|id| id.trim().is_empty())
        {
            return Err(EvalError::InvalidInput(
                "invalid public synthetic trace identity".into(),
            ));
        }
        for call in &self.calls {
            if !matches!(
                call.tool.as_str(),
                "assemble_context" | "ingest" | "extract" | "invalidate"
            ) || !call.arguments.is_object()
                || hash(&call.arguments)? != call.arguments_sha256
                || call.response.as_ref().map(hash).transpose()? != call.response_sha256
                || call.call_failed != call.response.is_none()
            {
                return Err(EvalError::InvalidInput(
                    "incomplete or inconsistent public synthetic call evidence".into(),
                ));
            }
        }
        Ok(())
    }

    fn validate_schema<M: MemoryToolSession>(&self, memory: &M) -> Result<(), EvalError> {
        if hash(memory.tool_schemas())? != self.identity.tool_schema_sha256 {
            return Err(EvalError::InvalidInput(
                "synthetic trace tool schema differs from active product".into(),
            ));
        }
        Ok(())
    }

    /// Execute the actual recorded arguments. Caller supplies an isolated product session.
    /// These are fresh diagnostic reads, not a regrade or fresh quality/latency claim.
    pub fn replay_for<M: MemoryToolSession>(
        &self,
        expected: &SyntheticTraceIdentity,
        memory: &mut M,
    ) -> Result<Vec<Value>, EvalError> {
        self.validate()?;
        if &self.identity != expected {
            return Err(EvalError::InvalidInput(
                "synthetic trace differs from registered replay identity".into(),
            ));
        }
        self.validate_schema(memory)?;
        self.calls
            .iter()
            .map(|call| memory.call_tool(&call.tool, call.arguments.clone()))
            .collect()
    }
}

fn hash(value: &Value) -> Result<String, EvalError> {
    Ok(sha256(&serde_json::to_vec(value)?))
}
