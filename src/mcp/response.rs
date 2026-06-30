pub(crate) use crate::tools::response::ToolResponse;

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
