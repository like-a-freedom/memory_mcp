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

    pub(crate) fn complete_list(
        result: T,
        total_count: usize,
        guidance: impl Into<String>,
    ) -> Self {
        Self {
            status: "success".to_string(),
            result,
            guidance: Some(guidance.into()),
            has_more: Some(false),
            total_count: Some(total_count),
            next_offset: None,
        }
    }

    /// Complete-list response in compact mode.
    ///
    /// `has_more` and `total_count` are set to `None` so the existing
    /// `skip_serializing_if = "Option::is_none"` omits them from the wire,
    /// cutting the redundant `has_more: false` / `total_count == len` metadata
    /// from compact payloads.
    pub(crate) fn complete_list_compact(
        result: T,
        _total_count: usize,
        guidance: impl Into<String>,
    ) -> Self {
        Self {
            status: "success".to_string(),
            result,
            guidance: Some(guidance.into()),
            has_more: None,
            total_count: None,
            next_offset: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
    fn complete_list_compact_omits_pagination_metadata() {
        let response = ToolResponse::complete_list_compact(vec!["a", "b"], 2, "done");
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["status"], "success");
        assert!(json.get("has_more").is_none(), "compact omits has_more");
        assert!(
            json.get("total_count").is_none(),
            "compact omits total_count"
        );
        assert!(json.get("next_offset").is_none());
        assert!(
            json["guidance"].as_str().unwrap().contains("done"),
            "guidance must survive compact mode"
        );
    }

    #[test]
    fn complete_list_non_compact_keeps_pagination() {
        let response = ToolResponse::complete_list(vec!["a"], 1, "done");
        let json = serde_json::to_value(&response).unwrap();
        assert_eq!(json["has_more"], false);
        assert_eq!(json["total_count"], 1);
    }
}
