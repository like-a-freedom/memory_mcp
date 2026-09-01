//! MCP protocol shaping for app sessions.
//!
//! Session state lives in [`crate::service::apps::session`]; this module
//! maps service results to `rmcp::ErrorData` and shapes protocol envelopes.

#![cfg_attr(not(feature = "mcp-apps"), allow(dead_code))]

use rmcp::ErrorData;
use serde_json::Value;

use super::resources::app_session_uri;
use super::response::OpenAppResult;

pub(crate) fn invalid_params(message: impl Into<String>) -> ErrorData {
    let msg = message.into();
    let data = serde_json::json!({
        "guidance": "Review the input arguments, fix any issues, and retry.",
    });
    ErrorData::invalid_params(msg, Some(data))
}

pub(crate) fn missing_app_field(app: &str, field: &str) -> ErrorData {
    let msg = format!("`{field}` is required for {app}");
    let data = serde_json::json!({
        "guidance": format!("Supply the `{field}` parameter and retry."),
    });
    ErrorData::invalid_params(msg, Some(data))
}

pub(crate) fn internal_error(message: impl Into<String>) -> ErrorData {
    let msg = message.into();
    let data = serde_json::json!({
        "guidance": "This is a transient error. Retry the operation.",
    });
    ErrorData::internal_error(msg, Some(data))
}

pub(crate) fn open_app_result(
    app: &str,
    session_id: impl Into<String>,
    fallback: Value,
) -> OpenAppResult {
    let session_id = session_id.into();
    OpenAppResult {
        app: app.to_string(),
        resource_uri: app_session_uri(app, &session_id),
        session_id,
        fallback,
    }
}

pub(crate) fn app_command_result_from_details(
    app: &str,
    session_id: &str,
    action: &str,
    resource_uri: Option<String>,
    details: Value,
) -> super::response::AppCommandResult {
    let ok = details.get("ok").and_then(Value::as_bool).unwrap_or(true);
    let message = details
        .get("message")
        .and_then(Value::as_str)
        .unwrap_or("done")
        .to_string();
    let refresh_required = details
        .get("refresh_required")
        .and_then(Value::as_bool)
        .unwrap_or(false);

    super::response::AppCommandResult {
        app: app.to_string(),
        session_id: session_id.to_string(),
        action: action.to_string(),
        ok,
        message,
        refresh_required,
        resource_uri,
        details: Some(details),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn open_app_result_builds_resource_uri() {
        let result = open_app_result("inspector", "ses:1", serde_json::json!({}));
        assert_eq!(result.resource_uri, "ui://memory/app/inspector/ses:1");
        assert_eq!(result.session_id, "ses:1");
    }

    #[test]
    fn app_command_result_defaults_ok_and_message() {
        let result = app_command_result_from_details(
            "diff",
            "ses:2",
            "export_diff",
            None,
            serde_json::json!({}),
        );
        assert!(result.ok);
        assert_eq!(result.message, "done");
        assert!(!result.refresh_required);
    }
}
