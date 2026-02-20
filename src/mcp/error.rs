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
#[must_use]
pub fn mcp_error(err: MemoryError) -> ErrorData {
    let code = match err {
        MemoryError::Validation(_) => ErrorCode::INVALID_PARAMS,
        MemoryError::NotFound(_) => ErrorCode::INVALID_PARAMS,
        MemoryError::ConfigMissing(_) => ErrorCode::INVALID_REQUEST,
        MemoryError::ConfigInvalid(_) => ErrorCode::INVALID_REQUEST,
        MemoryError::Storage(_) => ErrorCode::INTERNAL_ERROR,
    };
    ErrorData::new(code, err.to_string(), None)
}
