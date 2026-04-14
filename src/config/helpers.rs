//! Helper functions for parsing environment variables and path resolution.

use std::env;
use std::path::PathBuf;

use crate::service::MemoryError;

/// Parses a typed environment variable, returning `Ok(None)` when unset.
///
/// # Errors
///
/// Returns [`MemoryError::ConfigInvalid`] when the variable is set but cannot
/// be parsed as the target type.
pub fn parse_env<T: std::str::FromStr>(var_name: &str) -> Result<Option<T>, MemoryError> {
    env::var(var_name)
        .ok()
        .map(|value| {
            value
                .parse::<T>()
                .map_err(|_| MemoryError::ConfigInvalid(format!("{var_name} has an invalid value")))
        })
        .transpose()
}

/// Parses a boolean environment variable.
///
/// Recognizes "1", "true", "yes" (case-insensitive) as true.
pub fn parse_bool_env(var_name: &str) -> Option<bool> {
    env::var(var_name).ok().map(|v| {
        let v = v.to_lowercase();
        v == "1" || v == "true" || v == "yes"
    })
}

/// Parses a comma-separated list from an environment variable.
///
/// # Errors
///
/// Returns `MemoryError::ConfigMissing` if the variable is not set.
pub fn parse_comma_list(var_name: &str) -> Result<Vec<String>, MemoryError> {
    let raw = env::var(var_name).map_err(|_| MemoryError::ConfigMissing(var_name.to_string()))?;

    Ok(raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(String::from)
        .collect())
}

/// Returns the default embedded SurrealDB path.
///
/// If no explicit path is configured, we store DB files next to the running
/// executable to make runtime behavior independent from process working
/// directory.
pub fn default_embedded_data_dir() -> String {
    let base_dir = env::current_exe()
        .ok()
        .and_then(|path| path.parent().map(std::path::Path::to_path_buf))
        .or_else(|| env::current_dir().ok())
        .unwrap_or_else(|| PathBuf::from("."));

    base_dir
        .join("data")
        .join("surrealdb")
        .to_string_lossy()
        .to_string()
}
