/// Response wrapper for tool results.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub struct ToolResponse<T> {
    /// Result status for the tool call.
    pub status: String,
    /// The actual result data.
    pub result: T,
    /// Optional next-step guidance for the caller.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub guidance: Option<String>,
    /// Pagination flag for list responses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_more: Option<bool>,
    /// Total count of records in the current response slice.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_count: Option<usize>,
    /// Offset for the next page when pagination is supported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_offset: Option<usize>,
}

impl<T> ToolResponse<T> {
    pub(crate) fn success_with_guidance(result: T, guidance: impl Into<String>) -> Self {
        Self {
            status: "success".to_string(),
            result,
            guidance: Some(guidance.into()),
            has_more: None,
            total_count: None,
            next_offset: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn partial_with_guidance(result: T, guidance: impl Into<String>) -> Self {
        Self {
            status: "partial".to_string(),
            result,
            guidance: Some(guidance.into()),
            has_more: None,
            total_count: None,
            next_offset: None,
        }
    }

    pub(crate) fn complete_list(result: T, total_count: usize, guidance: impl Into<String>) -> Self {
        Self {
            status: "success".to_string(),
            result,
            guidance: Some(guidance.into()),
            has_more: Some(false),
            total_count: Some(total_count),
            next_offset: None,
        }
    }
}

/// Result payload returned by the public `open_app` launcher.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct OpenAppResult {
    /// Canonical public app identifier.
    pub app: String,
    /// Created session identifier.
    pub session_id: String,
    /// Session-backed resource URI for reading the current view.
    pub resource_uri: String,
    /// Immediate JSON fallback payload for clients that do not read resources yet.
    pub fallback: serde_json::Value,
}

/// Result payload returned by the public `app_command` bridge.
#[derive(Debug, serde::Serialize, schemars::JsonSchema)]
pub struct AppCommandResult {
    /// Canonical public app identifier.
    pub app: String,
    /// Target session identifier.
    pub session_id: String,
    /// Canonical action name.
    pub action: String,
    /// Whether the command completed successfully.
    pub ok: bool,
    /// Human-readable outcome message.
    pub message: String,
    /// Whether callers should re-read the session resource.
    pub refresh_required: bool,
    /// Resource URI to re-read when `refresh_required` is true.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resource_uri: Option<String>,
    /// Raw command details for clients that need extra metadata.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<serde_json::Value>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn tool_response_success_envelope_is_decision_ready() {
        let response = ToolResponse::success_with_guidance("ok", "next step");
        assert_eq!(response.status, "success");
        assert_eq!(response.result, "ok");
        assert_eq!(response.guidance.as_deref(), Some("next step"));
        assert!(response.has_more.is_none());
        assert!(response.total_count.is_none());
    }

    #[test]
    fn tool_response_complete_list_sets_all_pagination_fields() {
        let response = ToolResponse::complete_list(vec!["a", "b"], 2, "done");
        assert_eq!(response.status, "success");
        assert_eq!(response.has_more, Some(false));
        assert_eq!(response.total_count, Some(2));
    }

    #[test]
    fn tool_response_partial_envelope_marks_retryable_state() {
        let response = ToolResponse::partial_with_guidance("partial", "retry later");
        assert_eq!(response.status, "partial");
        assert_eq!(response.guidance.as_deref(), Some("retry later"));
    }

    #[test]
    fn tool_response_success_skips_pagination_fields() {
        let response = ToolResponse::success_with_guidance("ok", "done");
        let json = serde_json::to_value(&response).unwrap();
        assert!(json.get("has_more").is_none());
        assert!(json.get("total_count").is_none());
        assert!(json.get("next_offset").is_none());
    }

    #[test]
    fn app_command_result_serializes_all_fields() {
        let result = AppCommandResult {
            app: "test".into(),
            session_id: "s1".into(),
            action: "close_session".into(),
            ok: true,
            message: "closed".into(),
            refresh_required: false,
            resource_uri: None,
            details: Some(json!({"key": "value"})),
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["app"], "test");
        assert_eq!(json["ok"], true);
        assert_eq!(json["details"]["key"], "value");
    }

    #[test]
    fn app_command_result_omits_null_resource_uri() {
        let result = AppCommandResult {
            app: "test".into(),
            session_id: "s1".into(),
            action: "close_session".into(),
            ok: true,
            message: "closed".into(),
            refresh_required: false,
            resource_uri: None,
            details: None,
        };
        let json = serde_json::to_value(&result).unwrap();
        assert!(json.get("resource_uri").is_none());
    }

    #[test]
    fn open_app_result_serializes_correctly() {
        let result = OpenAppResult {
            app: "inspector".into(),
            session_id: "s1".into(),
            resource_uri: "mcp://app/inspector/s1".into(),
            fallback: json!({"key": "value"}),
        };
        let json = serde_json::to_value(&result).unwrap();
        assert_eq!(json["app"], "inspector");
        assert_eq!(json["session_id"], "s1");
    }
}
