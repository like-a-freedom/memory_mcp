//! Base-path aware same-origin URL construction.
//!
//! The console is served under the mount base `memory_mcp_http` stamps into
//! the document at startup — the same runtime value `main.rs` hands to
//! `dioxus_web::WebHistory` for router navigation. Every fetch and raw
//! `<a href>` goes through [`url`] so a prefixed deployment never issues a
//! root-absolute URL. Router `Link`s need nothing: `WebHistory` adds the
//! prefix itself.

/// The sentinel `dx bundle --base-path` bakes into this bundle instead of a
/// deployment prefix. `memory_mcp_http` replaces it in the served shell at
/// startup; values still carrying it mean "unstamped" and must never be used.
pub(crate) const BASE_PATH_SENTINEL: &str = "/__memory_mcp_base__";

/// The base path this bundle is served under (e.g. `/memory`), empty at root.
///
/// Resolved at runtime from the `DIOXUS_ASSET_ROOT` meta the server stamps,
/// with the compile-time `DIOXUS_ASSET_ROOT` as fallback — the exact source
/// `dioxus_web::WebHistory` would read on its own, filtered through
/// [`resolve_base`] so the unstamped sentinel can never leak into a request.
pub fn base_path() -> String {
    thread_local! {
        static BASE_PATH: std::cell::OnceCell<String> = const { std::cell::OnceCell::new() };
    }
    BASE_PATH.with(|cell| {
        cell.get_or_init(|| resolve_base(read_meta_base(), baked_base()))
            .clone()
    })
}

/// Pure precedence: the stamped meta wins, then the baked value, then the
/// origin root. Any value still carrying [`BASE_PATH_SENTINEL`] is skipped.
pub(crate) fn resolve_base(meta_content: Option<String>, baked: Option<String>) -> String {
    [meta_content, baked]
        .into_iter()
        .flatten()
        .find(|value| !value.contains(BASE_PATH_SENTINEL))
        .unwrap_or_default()
}

/// The stamped meta element (`<meta name="DIOXUS_ASSET_ROOT" content="…">`).
/// JS interop exists only on the wasm target; native tests resolve `None`.
fn read_meta_base() -> Option<String> {
    #[cfg(target_arch = "wasm32")]
    {
        dioxus_cli_config::get_meta_contents(dioxus_cli_config::ASSET_ROOT_ENV)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        None
    }
}

/// `dioxus_cli_config`'s own resolution (the `option_env!` compile-time
/// constant in release builds).
fn baked_base() -> Option<String> {
    dioxus_cli_config::base_path()
}

/// Join the bundle base and a root-absolute `path` into one same-origin URL.
pub fn url(path: &str) -> String {
    join(&base_path(), path)
}

/// Pure join: exactly one slash between base and path.
///
/// A leading `//` is not an absolute path — the URL spec reads it as a
/// scheme-relative reference (`scheme://api/…`), so the browser would look
/// up a host named `api` and reject the fetch before a byte is sent. The
/// same rule is pinned from the `ApiClient` side by its own tests.
pub fn join(base: &str, path: &str) -> String {
    let base = base.trim_end_matches('/');
    format!("{base}/{}", path.trim_start_matches('/'))
}

#[cfg(test)]
mod tests {
    use super::join;
    use super::{BASE_PATH_SENTINEL, resolve_base};

    #[test]
    fn the_stamped_meta_wins_over_the_baked_value() {
        assert_eq!(
            resolve_base(Some("/memory".to_string()), Some("/other".to_string())),
            "/memory"
        );
    }

    #[test]
    fn a_missing_meta_falls_back_to_the_baked_value() {
        assert_eq!(resolve_base(None, Some("/memory".to_string())), "/memory");
    }

    #[test]
    fn the_unstamped_sentinel_never_leaks_into_a_url() {
        // The release WASM bakes `option_env!("DIOXUS_ASSET_ROOT")` — the
        // sentinel itself when the bundle is relocatable. It must lose to
        // "no base at all", never reach a request URL.
        assert_eq!(resolve_base(Some(BASE_PATH_SENTINEL.to_string()), None), "");
        assert_eq!(resolve_base(None, Some(BASE_PATH_SENTINEL.to_string())), "");
    }

    #[test]
    fn root_deployments_resolve_to_the_empty_base() {
        assert_eq!(resolve_base(Some(String::new()), None), "");
        assert_eq!(resolve_base(None, None), "");
    }

    #[test]
    fn the_document_shell_carries_the_base_sentinel_in_literal_asset_urls() {
        // `dx bundle` prefixes only the tags it injects (css/js). Hand-written
        // hrefs in index.html survive verbatim (verified against dioxus-cli
        // 0.7.10), so they must carry the sentinel themselves or the favicon
        // escapes the server-side stamp.
        let shell = include_str!("../index.html");
        assert!(
            shell.contains(r#"href="/__memory_mcp_base__/assets/favicon.svg""#),
            "index.html literal asset URLs must carry /__memory_mcp_base__ so \
             memory_mcp_http can stamp them"
        );
    }

    #[test]
    fn empty_base_and_root_base_agree() {
        assert_eq!(join("", "/api/v1/account"), "/api/v1/account");
        assert_eq!(join("/", "/api/v1/account"), "/api/v1/account");
    }

    #[test]
    fn a_prefixed_base_prepends_exactly_one_slash() {
        assert_eq!(join("/memory", "/api/v1/account"), "/memory/api/v1/account");
        assert_eq!(join("/memory/", "api/v1/account"), "/memory/api/v1/account");
    }

    #[test]
    fn a_prefixed_join_is_never_scheme_relative() {
        assert!(!join("/memory/", "/api/v1/account").starts_with("//"));
    }
}
