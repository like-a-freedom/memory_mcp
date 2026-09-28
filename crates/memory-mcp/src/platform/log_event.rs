//! Structured log-event construction.
//!
//! Every operation in the system emits the same event shape, so the
//! shape is a platform concern rather than something each adapter
//! rebuilds. This is that shape.

use std::collections::HashMap;
use std::time::Duration;

use serde_json::{Value, json};

use crate::models::AccessPayload;

/// Builds a structured log event for tool operations.
pub fn log_event(
    op: &str,
    args: Value,
    result: Value,
    access: Option<&AccessPayload>,
    request_id: Option<&str>,
    duration_ms: Option<u64>,
) -> HashMap<String, Value> {
    let mut event = HashMap::new();
    event.insert("op".to_string(), Value::String(op.to_string()));
    event.insert("args".to_string(), args);
    event.insert("result".to_string(), result);
    if let Some(access) = access {
        event.insert("access".to_string(), serialize_access(access));
    }
    if let Some(rid) = request_id {
        event.insert("request_id".to_string(), Value::String(rid.to_string()));
    }
    if let Some(ms) = duration_ms {
        event.insert("duration_ms".to_string(), Value::Number(ms.into()));
    }
    event
}

pub(crate) fn serialize_access(access: &AccessPayload) -> Value {
    json!({
        "caller_id": access.caller_id,
        "allowed_tags": access.allowed_tags,
        "session_vars": access.session_vars,
        "transport": access.transport,
        "content_type": access.content_type,
    })
}

/// Adds a `duration_ms` field to an args Value.
#[must_use]
pub fn log_args_with_duration(mut args: Value, duration: Duration) -> Value {
    let duration_ms = u64::try_from(duration.as_millis()).unwrap_or(u64::MAX);
    if let Some(map) = args.as_object_mut() {
        map.insert("duration_ms".to_string(), json!(duration_ms));
        args
    } else {
        json!({ "value": args, "duration_ms": duration_ms })
    }
}

/// Builds a log result for embedding operations.
#[must_use]
pub(crate) fn build_embedding_log_result(
    generated_embeddings: usize,
    dimension: Option<usize>,
) -> Value {
    let mut result = serde_json::Map::new();
    result.insert(
        "generated_embeddings".to_string(),
        json!(generated_embeddings),
    );
    if let Some(dimension) = dimension {
        result.insert("dimension".to_string(), json!(dimension));
    }
    Value::Object(result)
}
