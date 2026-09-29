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

    #[test]
    fn app_command_result_honours_an_explicit_failure() {
        let observed = app_command_result_from_details(
            "diff",
            "ses:2",
            "export_diff",
            None,
            serde_json::json!({"ok": false, "message": "nothing to export"}),
        )
        .ok;

        assert!(!observed, "the service's verdict must not be overridden");
    }

    #[test]
    fn app_command_result_honours_an_explicit_message() {
        let observed = app_command_result_from_details(
            "diff",
            "ses:2",
            "export_diff",
            None,
            serde_json::json!({"message": "exported 3 changes"}),
        )
        .message;

        assert_eq!(observed, "exported 3 changes");
    }

    #[test]
    fn app_command_result_honours_an_explicit_refresh_request() {
        let observed = app_command_result_from_details(
            "diff",
            "ses:2",
            "export_diff",
            None,
            serde_json::json!({"refresh_required": true}),
        )
        .refresh_required;

        assert!(observed);
    }

    #[test]
    fn app_command_result_carries_the_resource_uri_through() {
        let observed = app_command_result_from_details(
            "diff",
            "ses:2",
            "export_diff",
            Some("ui://memory/app/diff/ses:2".to_string()),
            serde_json::json!({}),
        )
        .resource_uri;

        assert_eq!(observed.as_deref(), Some("ui://memory/app/diff/ses:2"));
    }

    #[test]
    fn app_command_result_keeps_the_raw_details() {
        let observed = app_command_result_from_details(
            "diff",
            "ses:2",
            "export_diff",
            None,
            serde_json::json!({"added": 2}),
        )
        .details;

        assert_eq!(observed.expect("details are retained")["added"], 2);
    }

    #[test]
    fn invalid_params_carries_retry_guidance() {
        let observed = invalid_params("bad input").data.expect("data is attached");

        assert!(observed["guidance"].is_string());
    }

    #[test]
    fn missing_app_field_names_the_field_in_its_message() {
        let observed = missing_app_field("diff", "session_id").message;

        assert!(
            observed.contains("session_id"),
            "the caller must be told which argument is missing: {observed}"
        );
    }

    #[test]
    fn missing_app_field_names_the_field_in_its_guidance() {
        let observed = missing_app_field("diff", "session_id")
            .data
            .expect("data is attached");

        assert_eq!(
            observed["guidance"],
            "Supply the `session_id` parameter and retry."
        );
    }

    #[test]
    fn internal_error_carries_retry_guidance() {
        let observed = internal_error("storage offline")
            .data
            .expect("data is attached");

        assert!(observed["guidance"].is_string());
    }

    #[test]
    fn open_app_result_carries_the_supplied_fallback() {
        let observed =
            open_app_result("inspector", "ses:1", serde_json::json!({"rows": []})).fallback;

        assert_eq!(observed["rows"].as_array().map(Vec::len), Some(0));
    }
}
