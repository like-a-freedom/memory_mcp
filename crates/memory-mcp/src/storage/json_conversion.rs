//! Consuming conversion from SurrealDB's typed result values to JSON.
//!
//! Numbers and containers are converted directly so they do not first allocate
//! a second, SDK-tagged JSON tree. Rare SDK values keep their existing tagged
//! representation to preserve the storage adapter's JSON contract.

use serde_json::{Number as JsonNumber, Value as JsonValue};
use surrealdb::types::{Number as SurrealNumber, Value as SurrealValue};

/// Converts a query result into rows while preserving the adapter's result
/// shape: `NONE` has no rows, arrays contain rows, and every other value is one
/// row.
pub(crate) fn surreal_records_to_json(value: SurrealValue) -> Vec<JsonValue> {
    match value {
        SurrealValue::None => Vec::new(),
        SurrealValue::Array(values) => values.into_iter().map(surreal_to_json).collect(),
        other => vec![surreal_to_json(other)],
    }
}

/// Consumes one typed SurrealDB value into its storage adapter JSON form.
pub(crate) fn surreal_to_json(value: SurrealValue) -> JsonValue {
    match value {
        SurrealValue::None | SurrealValue::Null => JsonValue::Null,
        SurrealValue::Bool(value) => JsonValue::Bool(value),
        SurrealValue::Number(SurrealNumber::Int(value)) => {
            JsonValue::Number(JsonNumber::from(value))
        }
        SurrealValue::Number(SurrealNumber::Float(value)) => JsonNumber::from_f64(value)
            .map(JsonValue::Number)
            .unwrap_or(JsonValue::Null),
        SurrealValue::Number(SurrealNumber::Decimal(value)) => JsonValue::String(value.to_string()),
        SurrealValue::String(value) => JsonValue::String(value),
        SurrealValue::Datetime(value) => JsonValue::String(value.to_string()),
        SurrealValue::Array(values) => {
            JsonValue::Array(values.into_iter().map(surreal_to_json).collect())
        }
        SurrealValue::Object(values) => JsonValue::Object(
            values
                .into_iter()
                .map(|(key, value)| (key, surreal_to_json(value)))
                .collect(),
        ),
        // Keep the original serde representation for RecordId and uncommon
        // values, then apply the same tag normalization as the old adapter
        // path. This allocates only the rare leaf's tree, never the numeric
        // arrays and objects that dominate query results.
        rare_or_tagged => {
            let tagged = serde_json::to_value(rare_or_tagged).unwrap_or(JsonValue::Null);
            super::helpers::normalize_surreal_json(&tagged)
        }
    }
}
