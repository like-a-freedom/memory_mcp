//! Helper functions for parsing environment variables and path resolution.

use std::env;
use std::path::{Path, PathBuf};

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

/// Returns whether a URL selects a supported remote SurrealDB connection.
pub(crate) fn is_remote_url(url: Option<&str>) -> bool {
    let Some(raw) = url.map(str::trim) else {
        return false;
    };
    let Some((scheme, authority_and_path)) = raw.split_once("://") else {
        return false;
    };
    let authority = authority_and_path
        .split(['/', '?', '#'])
        .next()
        .unwrap_or_default();
    if authority.is_empty() || authority_and_path.chars().any(char::is_whitespace) {
        return false;
    }
    matches!(
        scheme.to_ascii_lowercase().as_str(),
        "ws" | "wss" | "http" | "https"
    )
}

/// Normalizes only the scheme and surrounding whitespace of a URL-like value.
pub(crate) fn normalize_url_scheme(raw: &str) -> String {
    let trimmed = raw.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return trimmed.to_string();
    };
    format!("{}://{rest}", scheme.to_ascii_lowercase())
}

fn default_user_data_dir_from_env(
    xdg_data_home: Option<&str>,
    home: Option<&str>,
    current_dir: Option<&str>,
) -> String {
    if let Some(base) = xdg_data_home.filter(|value| !value.is_empty()) {
        return PathBuf::from(base)
            .join("memory_mcp")
            .to_string_lossy()
            .into_owned();
    }
    if let Some(base) = home.filter(|value| !value.is_empty()) {
        return PathBuf::from(base)
            .join(".local")
            .join("share")
            .join("memory_mcp")
            .to_string_lossy()
            .into_owned();
    }
    PathBuf::from(current_dir.unwrap_or("."))
        .join(".memory_mcp")
        .to_string_lossy()
        .into_owned()
}

/// Returns the new user-owned default data directory.
pub(crate) fn default_user_data_dir() -> String {
    let xdg_data_home = env::var("XDG_DATA_HOME").ok();
    let home = env::var("HOME").ok();
    let current_dir = env::current_dir()
        .ok()
        .map(|path| path.to_string_lossy().into_owned());

    default_user_data_dir_from_env(
        xdg_data_home.as_deref(),
        home.as_deref(),
        current_dir.as_deref(),
    )
}

/// Returns the legacy executable-relative data directory candidate.
pub(crate) fn legacy_embedded_data_dir_from_exe(exe_path: Option<&Path>) -> Option<String> {
    exe_path
        .and_then(Path::parent)
        .filter(|parent| !parent.as_os_str().is_empty())
        .map(|parent| {
            parent
                .join("data")
                .join("surrealdb")
                .to_string_lossy()
                .into_owned()
        })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct EmbeddedDataDirResolution {
    pub(crate) path: String,
    pub(crate) legacy_path: Option<String>,
}

fn select_embedded_data_dir(
    new_path: String,
    legacy_path: Option<String>,
    new_exists: bool,
    legacy_exists: bool,
) -> EmbeddedDataDirResolution {
    if new_exists {
        return EmbeddedDataDirResolution {
            path: new_path,
            legacy_path: None,
        };
    }

    if legacy_exists && let Some(legacy_path) = legacy_path {
        return EmbeddedDataDirResolution {
            path: legacy_path.clone(),
            legacy_path: Some(legacy_path),
        };
    }

    EmbeddedDataDirResolution {
        path: new_path,
        legacy_path: None,
    }
}

pub(crate) fn resolve_embedded_data_dir() -> EmbeddedDataDirResolution {
    let new_path = default_user_data_dir();
    let executable = env::current_exe().ok();
    let legacy_path = legacy_embedded_data_dir_from_exe(executable.as_deref());
    let new_exists = Path::new(&new_path).is_dir();
    let legacy_exists = legacy_path
        .as_deref()
        .is_some_and(|path| Path::new(path).is_dir());

    select_embedded_data_dir(new_path, legacy_path, new_exists, legacy_exists)
}

/// Returns the compatibility-selected embedded SurrealDB data directory.
pub fn default_embedded_data_dir() -> String {
    resolve_embedded_data_dir().path
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    #[test]
    fn remote_url_detection_accepts_only_remote_schemes() {
        assert!(is_remote_url(Some("ws://localhost:8000")));
        assert!(is_remote_url(Some("wss://db.example.com")));
        assert!(is_remote_url(Some("http://localhost:8000")));
        assert!(is_remote_url(Some("https://db.example.com")));
        assert!(!is_remote_url(None));
        assert!(!is_remote_url(Some("mem://local")));
        assert!(!is_remote_url(Some("rocksdb://local")));
        assert!(!is_remote_url(Some("not a URL")));
        assert!(!is_remote_url(Some("ws://")));
        assert!(!is_remote_url(Some("https://")));
        assert!(!is_remote_url(Some("https:///path")));
        assert!(!is_remote_url(Some("https://?query")));
        assert!(!is_remote_url(Some(
            "https://db.example.com/path with space"
        )));
        assert!(is_remote_url(Some("  https://db.example.com  ")));
        assert!(is_remote_url(Some("HTTPS://db.example.com")));
        assert!(!is_remote_url(Some("")));
    }

    #[test]
    fn url_scheme_normalization_trims_and_lowercases_only_the_scheme() {
        assert_eq!(
            normalize_url_scheme("  HTTPS://db.example.com/path  "),
            "https://db.example.com/path"
        );
        assert_eq!(normalize_url_scheme("mem://local"), "mem://local");
        assert_eq!(normalize_url_scheme("relative/path"), "relative/path");
    }

    #[test]
    fn default_user_data_dir_prefers_xdg_data_home() {
        let path =
            default_user_data_dir_from_env(Some("/tmp/xdg-data"), Some("/Users/alice"), None);
        assert_eq!(
            PathBuf::from(path),
            PathBuf::from("/tmp/xdg-data").join("memory_mcp")
        );
    }

    #[test]
    fn default_user_data_dir_uses_home_when_xdg_is_unset() {
        let path = default_user_data_dir_from_env(None, Some("/Users/alice"), None);
        assert_eq!(
            PathBuf::from(path),
            PathBuf::from("/Users/alice")
                .join(".local")
                .join("share")
                .join("memory_mcp")
        );
    }

    #[test]
    fn default_user_data_dir_has_deterministic_fallback_without_home() {
        let path = default_user_data_dir_from_env(None, None, Some("/tmp/worktree"));
        assert_eq!(
            PathBuf::from(path),
            PathBuf::from("/tmp/worktree").join(".memory_mcp")
        );
    }

    #[test]
    fn embedded_data_dir_selector_prefers_new_path_when_both_exist() {
        let resolution = select_embedded_data_dir(
            "/new/memory_mcp".to_string(),
            Some("/legacy/data/surrealdb".to_string()),
            true,
            true,
        );
        assert_eq!(resolution.path, "/new/memory_mcp");
        assert!(resolution.legacy_path.is_none());
    }

    #[test]
    fn embedded_data_dir_selector_falls_back_to_existing_legacy_path() {
        let resolution = select_embedded_data_dir(
            "/new/memory_mcp".to_string(),
            Some("/legacy/data/surrealdb".to_string()),
            false,
            true,
        );
        assert_eq!(resolution.path, "/legacy/data/surrealdb");
        assert_eq!(
            resolution.legacy_path.as_deref(),
            Some("/legacy/data/surrealdb")
        );
    }

    #[test]
    fn embedded_data_dir_selector_uses_new_path_on_fresh_install() {
        let resolution = select_embedded_data_dir(
            "/new/memory_mcp".to_string(),
            Some("/legacy/data/surrealdb".to_string()),
            false,
            false,
        );
        assert_eq!(resolution.path, "/new/memory_mcp");
        assert!(resolution.legacy_path.is_none());
    }

    #[test]
    fn embedded_data_dir_selector_does_not_report_missing_legacy_candidate() {
        let resolution = select_embedded_data_dir("/new/memory_mcp".to_string(), None, false, true);
        assert_eq!(resolution.path, "/new/memory_mcp");
        assert!(resolution.legacy_path.is_none());
    }

    #[test]
    fn legacy_embedded_data_dir_uses_executable_parent() {
        let path = legacy_embedded_data_dir_from_exe(Some(Path::new("/opt/bin/memory_mcp")));
        assert_eq!(path.as_deref(), Some("/opt/bin/data/surrealdb"));
        assert!(legacy_embedded_data_dir_from_exe(None).is_none());
    }
}
