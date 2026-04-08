//! Error conversion utilities for MCP protocol.

use rmcp::ErrorData;
use rmcp::model::ErrorCode;

use crate::service::MemoryError;

/// Converts a `MemoryError` into an MCP `ErrorData` response.
///
/// Maps error types to appropriate MCP error codes:
/// - `Validation` → `INVALID_PARAMS`
/// - `NotFound` → `INVALID_PARAMS`
/// - `ConfigMissing` → `INVALID_REQUEST`
/// - `ConfigInvalid` → `INVALID_REQUEST`
/// - `Storage` → `INTERNAL_ERROR`
pub fn mcp_error(err: MemoryError) -> ErrorData {
    let (code, guidance) = match &err {
        MemoryError::Validation(_) => (
            ErrorCode::INVALID_PARAMS,
            "Fix the input arguments and retry.",
        ),
        MemoryError::NotFound(_) => (
            ErrorCode::INVALID_PARAMS,
            "Verify the identifier or create the missing memory record first.",
        ),
        MemoryError::ConfigMissing(_) | MemoryError::ConfigInvalid(_) => (
            ErrorCode::INVALID_REQUEST,
            "Fix the server configuration before retrying this tool call.",
        ),
        MemoryError::Storage(_) => (
            ErrorCode::INTERNAL_ERROR,
            "Retry the request. If the problem persists, inspect server logs.",
        ),
    };
    ErrorData::new(code, format!("{} Guidance: {guidance}", err), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mcp_error_maps_validation_to_invalid_params() {
        let err = MemoryError::Validation("test error".to_string());
        let mcp_err = mcp_error(err);
        assert_eq!(mcp_err.code, ErrorCode::INVALID_PARAMS);
        assert!(mcp_err.message.contains("test error"));
    }

    #[test]
    fn mcp_error_maps_not_found_to_invalid_params() {
        let err = MemoryError::NotFound("resource not found".to_string());
        let mcp_err = mcp_error(err);
        assert_eq!(mcp_err.code, ErrorCode::INVALID_PARAMS);
        assert!(mcp_err.message.contains("resource not found"));
    }

    #[test]
    fn mcp_error_maps_config_missing_to_invalid_request() {
        let err = MemoryError::ConfigMissing("SURREALDB_URL".to_string());
        let mcp_err = mcp_error(err);
        assert_eq!(mcp_err.code, ErrorCode::INVALID_REQUEST);
        assert!(mcp_err.message.contains("SURREALDB_URL"));
    }

    #[test]
    fn mcp_error_maps_config_invalid_to_invalid_request() {
        let err = MemoryError::ConfigInvalid("invalid value".to_string());
        let mcp_err = mcp_error(err);
        assert_eq!(mcp_err.code, ErrorCode::INVALID_REQUEST);
        assert!(mcp_err.message.contains("invalid value"));
    }

    #[test]
    fn mcp_error_maps_storage_to_internal_error() {
        let err = MemoryError::Storage("database error".to_string());
        let mcp_err = mcp_error(err);
        assert_eq!(mcp_err.code, ErrorCode::INTERNAL_ERROR);
        assert!(mcp_err.message.contains("database error"));
    }

    #[test]
    fn mcp_error_includes_error_message() {
        let err = MemoryError::Validation("field is required".to_string());
        let mcp_err = mcp_error(err);
        assert_eq!(
            mcp_err.message,
            "validation error: field is required Guidance: Fix the input arguments and retry."
        );
    }

    #[test]
    fn mcp_error_has_none_data() {
        let err = MemoryError::NotFound("test".to_string());
        let mcp_err = mcp_error(err);
        assert!(mcp_err.data.is_none());
    }
}
