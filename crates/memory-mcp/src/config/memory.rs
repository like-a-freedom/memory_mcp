//! Memory-retention limits resolved by a process composition root.

use std::num::NonZeroUsize;

use crate::error::MemoryError;

pub const DEFAULT_HTTP_CONTEXT_CACHE_BYTES: usize = 4 * 1024 * 1024;
pub const DEFAULT_LOCAL_CONTEXT_CACHE_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_QUERY_EMBEDDING_CACHE_BYTES: usize = 2 * 1024 * 1024;

/// Byte budgets for process-local caches. These estimates bound retained
/// allocations, not exact allocator RSS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheLimits {
    pub context_bytes: NonZeroUsize,
    pub query_bytes: NonZeroUsize,
}

impl CacheLimits {
    /// Resolves the memory-cache environment variables once at startup.
    pub fn from_env(default_context_bytes: usize) -> Result<Self, MemoryError> {
        let context_bytes = super::helpers::parse_env("MEMORY_CONTEXT_CACHE_BYTES")?;
        let query_bytes = super::helpers::parse_env("MEMORY_QUERY_EMBEDDING_CACHE_BYTES")?;
        Self::from_values(default_context_bytes, context_bytes, query_bytes)
    }

    /// Returns non-zero profile defaults without consulting the environment.
    #[must_use]
    pub const fn profile_default() -> Self {
        #[cfg(feature = "streamable-http")]
        let context_bytes = DEFAULT_HTTP_CONTEXT_CACHE_BYTES;
        #[cfg(not(feature = "streamable-http"))]
        let context_bytes = DEFAULT_LOCAL_CONTEXT_CACHE_BYTES;
        Self {
            context_bytes: nonzero_or_min(context_bytes),
            query_bytes: nonzero_or_min(DEFAULT_QUERY_EMBEDDING_CACHE_BYTES),
        }
    }

    pub(crate) fn from_values(
        default_context_bytes: usize,
        context_override: Option<usize>,
        query_override: Option<usize>,
    ) -> Result<Self, MemoryError> {
        let context_bytes = context_override.unwrap_or(default_context_bytes);
        let query_bytes = query_override.unwrap_or(DEFAULT_QUERY_EMBEDDING_CACHE_BYTES);
        Ok(Self {
            context_bytes: NonZeroUsize::new(context_bytes).ok_or_else(|| {
                MemoryError::ConfigInvalid(
                    "MEMORY_CONTEXT_CACHE_BYTES must be greater than zero".to_string(),
                )
            })?,
            query_bytes: NonZeroUsize::new(query_bytes).ok_or_else(|| {
                MemoryError::ConfigInvalid(
                    "MEMORY_QUERY_EMBEDDING_CACHE_BYTES must be greater than zero".to_string(),
                )
            })?,
        })
    }
}

const fn nonzero_or_min(value: usize) -> NonZeroUsize {
    match NonZeroUsize::new(value) {
        Some(nonzero) => nonzero,
        None => NonZeroUsize::MIN,
    }
}

#[cfg(test)]
mod tests {
    use super::CacheLimits;

    #[test]
    fn profile_defaults_distinguish_http_from_local_context_budget() {
        let limits = CacheLimits::profile_default();
        let expected_context_bytes = if cfg!(feature = "streamable-http") {
            4 * 1024 * 1024
        } else {
            16 * 1024 * 1024
        };

        assert_eq!(limits.context_bytes.get(), expected_context_bytes);
        assert_eq!(limits.query_bytes.get(), 2 * 1024 * 1024);
    }

    #[test]
    fn cache_limits_use_profile_context_default_and_shared_query_default() {
        let limits = CacheLimits::from_values(16 * 1024 * 1024, None, None)
            .expect("positive defaults are valid");

        assert_eq!(limits.context_bytes.get(), 16 * 1024 * 1024);
        assert_eq!(limits.query_bytes.get(), 2 * 1024 * 1024);
    }

    #[test]
    fn cache_limits_accept_explicit_byte_overrides() {
        let limits = CacheLimits::from_values(16 * 1024 * 1024, Some(4_194_304), Some(2_097_152))
            .expect("positive overrides are valid");

        assert_eq!(limits.context_bytes.get(), 4_194_304);
        assert_eq!(limits.query_bytes.get(), 2_097_152);
    }

    #[test]
    fn cache_limits_reject_zero_context_budget() {
        let result = CacheLimits::from_values(16 * 1024 * 1024, Some(0), None);

        assert!(result.is_err());
    }

    #[test]
    fn cache_limits_reject_zero_query_budget() {
        let result = CacheLimits::from_values(16 * 1024 * 1024, None, Some(0));

        assert!(result.is_err());
    }
}
