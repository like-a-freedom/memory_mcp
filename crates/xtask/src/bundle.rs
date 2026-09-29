//! The console bundle's shape contract.
//!
//! `crates/memory-mcp/build.rs` embeds whatever this directory contains and
//! `crates/ui` boots from it in a browser, so two properties have to hold or
//! the image ships a console that cannot start:
//!
//!   * exactly one of each runtime artifact — a stale staging directory left
//!     over from an earlier `dx bundle` keeps the previous build's files, and
//!     the binary then embeds both pairs;
//!   * the base-path sentinel is present, because `memory_mcp_http` stamps the
//!     deployed prefix over it at startup and a bundle built with a literal
//!     prefix cannot be relocated.
//!
//! This ran as shell `test`/`grep` lines inside the Dockerfile and was pinned
//! by a Python test. Both are now one command.

use std::fs;
use std::path::Path;

use crate::pack::PackError;

/// The literal `crates/ui` writes into every prefix-dependent URL. It must
/// match `BASE_PATH_SENTINEL` in `crates/memory-mcp/src/control/static_assets.rs`
/// and `crates/ui/src/base.rs`.
pub const BASE_PATH_SENTINEL: &str = "/__memory_mcp_base__";

/// One file per kind: a bundle carrying a second `.wasm` is carrying a
/// previous build's.
const SINGLETON_SUFFIXES: [(&str, &str); 3] =
    [("js", "script"), ("wasm", "module"), ("css", "stylesheet")];

/// Require `dist` to be a bundle `build.rs` can embed and a browser can boot.
pub fn check(dist: &Path) -> Result<(), PackError> {
    if !dist.is_absolute() {
        return Err(PackError::Bundle(format!(
            "the bundle path must be absolute; received {}",
            dist.display()
        )));
    }
    if !dist.is_dir() {
        return Err(PackError::Bundle(format!(
            "{} is not a directory",
            dist.display()
        )));
    }

    let index = dist.join("index.html");
    let html = fs::read_to_string(&index)
        .map_err(|e| PackError::Bundle(format!("cannot read {}: {e}", index.display())))?;
    if html.trim().is_empty() {
        return Err(PackError::Bundle(format!(
            "{} is empty; the console would boot to a blank page",
            index.display()
        )));
    }
    if !html.contains(BASE_PATH_SENTINEL) {
        return Err(PackError::Bundle(format!(
            "{} does not carry {BASE_PATH_SENTINEL}, so the bundle cannot be \
             relocated to a deployment prefix",
            index.display()
        )));
    }

    let files = walk(dist)?;
    for (suffix, kind) in SINGLETON_SUFFIXES {
        let matches: Vec<&std::path::Path> = files
            .iter()
            .map(|path| path.as_path())
            .filter(|path| {
                path.extension()
                    .and_then(|extension| extension.to_str())
                    .is_some_and(|extension| extension.eq_ignore_ascii_case(suffix))
            })
            .collect();
        if matches.len() != 1 {
            return Err(PackError::Bundle(format!(
                "the bundle holds {} {kind} file(s) at {}, expected exactly one; a \
                 stale staging directory keeps the previous build's files",
                matches.len(),
                dist.display()
            )));
        }
    }
    Ok(())
}

fn walk(dir: &Path) -> Result<Vec<std::path::PathBuf>, PackError> {
    let mut found = Vec::new();
    let mut pending = vec![dir.to_path_buf()];
    while let Some(current) = pending.pop() {
        let entries = fs::read_dir(&current)
            .map_err(|e| PackError::Bundle(format!("cannot read {}: {e}", current.display())))?;
        for entry in entries {
            let entry = entry.map_err(|e| {
                PackError::Bundle(format!("cannot enumerate {}: {e}", current.display()))
            })?;
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else if path.is_file() {
                found.push(path);
            }
        }
    }
    found.sort();
    Ok(found)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bundle(entries: &[(&str, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (name, body) in entries {
            let path = dir.path().join(name);
            if let Some(parent) = path.parent() {
                fs::create_dir_all(parent).expect("create the asset's directory");
            }
            fs::write(path, body).expect("write");
        }
        dir
    }

    fn valid() -> tempfile::TempDir {
        bundle(&[
            (
                "index.html",
                &format!("<base href=\"{BASE_PATH_SENTINEL}\">"),
            ),
            ("assets/app.js", "console"),
            ("assets/app.wasm", "module"),
            ("assets/app.css", "body{}"),
        ])
    }

    #[test]
    fn a_well_formed_bundle_passes() {
        let dir = valid();
        check(dir.path()).expect("the bundle is complete");
    }

    /// The failure this exists for: `dx bundle` writes into a staging
    /// directory that a cached build leaves behind, so the next build embeds
    /// two of everything.
    #[test]
    fn a_stale_second_module_is_rejected() {
        let dir = bundle(&[
            ("index.html", BASE_PATH_SENTINEL),
            ("a.wasm", "old"),
            ("b.wasm", "new"),
            ("a.js", "j"),
            ("a.css", "c"),
        ]);
        let error = check(dir.path()).expect_err("two wasm files");
        assert!(
            error.to_string().contains("expected exactly one"),
            "{error}"
        );
    }

    #[test]
    fn a_bundle_without_the_sentinel_cannot_be_relocated() {
        let dir = bundle(&[
            ("index.html", "<base href=\"/console\">"),
            ("a.wasm", "m"),
            ("a.js", "j"),
            ("a.css", "c"),
        ]);
        let error = check(dir.path()).expect_err("no sentinel");
        assert!(error.to_string().contains(BASE_PATH_SENTINEL), "{error}");
    }

    #[test]
    fn an_empty_index_is_rejected() {
        let dir = bundle(&[
            ("index.html", ""),
            ("a.wasm", "m"),
            ("a.js", "j"),
            ("a.css", "c"),
        ]);
        assert!(check(dir.path()).is_err());
    }

    #[test]
    fn a_missing_module_is_rejected() {
        let dir = bundle(&[
            ("index.html", BASE_PATH_SENTINEL),
            ("a.js", "j"),
            ("a.css", "c"),
        ]);
        let error = check(dir.path()).expect_err("no wasm");
        assert!(error.to_string().contains("module"), "{error}");
    }
}
