//! Base-path aware same-origin URL construction.
//!
//! The console is served under the path prefix baked by
//! `dx bundle --base-path` — the same native source `dioxus_web::WebHistory`
//! reads for router navigation. Every fetch and raw `<a href>` goes through
//! [`url`] so a prefixed deployment never issues a root-absolute URL.
//! Router `Link`s need nothing: `WebHistory` adds the prefix itself.

/// The base path this bundle was built for (e.g. `/memory`), empty at root.
pub fn base_path() -> String {
    dioxus_cli_config::base_path().unwrap_or_default()
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
