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
/// match `BASE_PATH_SENTINEL` in `crates/memory-mcp/src/ui/assets.rs`, the
/// favicon href in `crates/ui/index.html`, and the `--base-path` argument in
/// the `Dockerfile`'s `dx bundle` invocation.
pub const BASE_PATH_SENTINEL: &str = "/__memory_mcp_base__";

/// The prefix `crates/memory-mcp/build.rs` namespaces gzip sidecars under.
///
/// It is a prefix rather than a `.gz` suffix because a suffix collides: a
/// bundle carrying both `app.wasm` and a file named `app.wasm.gz` would stage
/// both onto one path, and the binary would serve one asset's bytes under the
/// other's name. A bundle file that starts with this prefix would collide just
/// as badly, so the bundle is refused here rather than discovered as a corrupt
/// console at runtime.
pub const GZIP_PREFIX: &str = "gzip__";

/// One file per kind: a bundle carrying a second `.wasm` is carrying a
/// previous build's.
const SINGLETON_SUFFIXES: [(&str, &str); 3] =
    [("js", "script"), ("wasm", "module"), ("css", "stylesheet")];

/// The console's framework pin, and the image's CLI pin. Neither file mentions
/// the other, so nothing but this check relates them.
const UI_MANIFEST: &str = "crates/ui/Cargo.toml";
const DOCKERFILE: &str = "Dockerfile";

/// Require the Dioxus CLI the image installs to be the version the UI crate
/// compiles against.
///
/// The CLI bundles the framework it carries, so an image shipping one version
/// while the crate pins another builds a bundle that pin never verified. The
/// Python check that used to assert this was removed with the rest of
/// `scripts/ci`; this is its replacement, and the reason the pin is still
/// checked at all.
pub fn check_cli_pin() -> Result<(), PackError> {
    let dockerfile = read_repository_file(DOCKERFILE)?;
    let pinned = dioxus_cli_arg(&dockerfile).ok_or_else(|| {
        PackError::Bundle(format!("{DOCKERFILE} no longer pins DIOXUS_CLI_VERSION"))
    })?;
    let manifest = read_repository_file(UI_MANIFEST)?;
    let required = dioxus_requirement(&manifest).ok_or_else(|| {
        PackError::Bundle(format!("{UI_MANIFEST} no longer declares a dioxus version"))
    })?;

    if pinned != required {
        return Err(PackError::Bundle(format!(
            "the image would build the console with dioxus-cli {pinned}, but the UI \
             crate pins dioxus ={required}. The CLI carries the framework it \
             compiles against, so the two must be the same version."
        )));
    }
    check_web_strip_profile()
}

/// Require the profile `dx bundle` builds the console with to keep wasm-bindgen's
/// metadata intact.
///
/// The CLI builds the web target under the profile named `wasm-release`
/// (`Platform::profile_name`), reads that profile out of the workspace manifest,
/// passes `strip=false` to cargo so the linker keeps its sections, and then runs
/// `rust-objcopy` on the module itself *before* wasm-bindgen. `strip = true`
/// resolves to `--strip-all`, which since LLVM 23 removes every custom section —
/// including `__wasm_bindgen_unstable`, the metadata wasm-bindgen reads to emit
/// the JS glue. The build then dies with "the `__wasm_bindgen_unstable` custom
/// section is missing", and wasm-opt fails afterwards on a module that was never
/// written.
///
/// The symptom is far from the cause: it surfaces inside `wasm-opt`, names a
/// file that legitimately does not exist yet, and only under an LLVM recent
/// enough to have changed `--strip-all`. So the profile that decides it is
/// checked here, where the other console-build contract is.
fn check_web_strip_profile() -> Result<(), PackError> {
    let manifest = read_repository_file(WORKSPACE_MANIFEST)?;
    check_web_strip_profile_for(&manifest)
}

/// `check_web_strip_profile` over a manifest's text, so the rule can be exercised
/// without the repository.
fn check_web_strip_profile_for(manifest: &str) -> Result<(), PackError> {
    let setting = web_profile_strip(manifest).ok_or_else(|| {
        PackError::Bundle(format!(
            "{WORKSPACE_MANIFEST} declares no [profile.wasm-release], so `dx bundle` \
             falls back to [profile.release] for the console"
        ))
    })?;

    if setting == STRIP_SYMBOLS {
        return Err(PackError::Bundle(format!(
            "[profile.wasm-release] sets strip = {STRIP_SYMBOLS}, which makes `dx \
             bundle` run `rust-objcopy --strip-all` on the console module before \
             wasm-bindgen. Since LLVM 23 that deletes the \
             `__wasm_bindgen_unstable` custom section wasm-bindgen requires, and \
             the build fails. Use strip = \"debuginfo\", which drops the DWARF and \
             keeps the custom sections."
        )));
    }
    Ok(())
}

/// The `strip` value of `[profile.wasm-release]`, or `None` when the manifest
/// declares no such profile.
fn web_profile_strip(manifest: &str) -> Option<String> {
    section_body(manifest, "[profile.wasm-release]")
        .lines()
        .map(str::trim)
        .filter(|line| !line.starts_with('#') && !line.is_empty())
        .find_map(|line| line.strip_prefix("strip")?.trim_start().strip_prefix('='))
        .map(|value| value.trim().trim_matches('"').to_owned())
}

/// The lines of a top-level `[section]` in a manifest, up to the next section.
///
/// Comments are left in place; callers filter them. Only a top-level section
/// ends the body, so a nested `[profile.release.package."*"]` does not truncate
/// the profile above it.
fn section_body<'a>(manifest: &'a str, header: &str) -> &'a str {
    let Some(start) = manifest.find(header) else {
        return "";
    };
    let rest = &manifest[start + header.len()..];
    // Byte offsets, not line counts: only the first `line` of a match is
    // guaranteed to start where the match starts.
    let end = rest
        .match_indices('\n')
        .find(|(_, after)| after.starts_with('['))
        .map_or(rest.len(), |(offset, _)| offset);
    &rest[..end]
}

/// Cargo's `strip = true`, the spelling that means `--strip-all`.
const STRIP_SYMBOLS: &str = "true";

/// The `Cargo.toml` whose profiles `dx bundle` reads.
const WORKSPACE_MANIFEST: &str = "Cargo.toml";

/// The `DIOXUS_CLI_VERSION` the `Dockerfile` installs.
fn dioxus_cli_arg(dockerfile: &str) -> Option<String> {
    dockerfile
        .lines()
        .filter_map(|line| line.trim().strip_prefix("ARG DIOXUS_CLI_VERSION="))
        .find_map(|value| value.split_whitespace().next())
        .map(str::to_owned)
}

/// The version the UI crate requires, without the `=` an exact requirement
/// carries.
fn dioxus_requirement(manifest: &str) -> Option<String> {
    let line = manifest
        .lines()
        .find(|line| line.trim_start().starts_with("dioxus") && line.contains("version"))?;
    let value = line.split_once("version")?.1;
    let version = value.split('"').nth(1)?;
    Some(version.trim_start_matches('=').to_owned())
}

/// Read a repository file, resolved from this crate's manifest directory so the
/// command works from any working directory.
fn read_repository_file(relative: &str) -> Result<String, PackError> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .ok_or_else(|| {
            PackError::Bundle("cannot locate the repository root from the manifest".into())
        })?;
    let path = root.join(relative);
    fs::read_to_string(&path)
        .map_err(|error| PackError::Bundle(format!("cannot read {}: {error}", path.display())))
}

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
    // A bundle file squatting the gzip sidecar namespace would be staged onto
    // the same path as the sidecar `build.rs` writes for its sibling, and the
    // binary would serve one asset's bytes under the other's name. Nothing
    // downstream can catch that — the catalog compiles and the console fails
    // to boot at runtime — so it is refused here.
    if let Some(squatted) = files.iter().map(|path| path.as_path()).find(|path| {
        path.file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(GZIP_PREFIX))
    }) {
        return Err(PackError::Bundle(format!(
            "the bundle contains {}, whose name starts with the reserved {GZIP_PREFIX} \
             prefix that `build.rs` namespaces gzip sidecars under; rename it",
            squatted.display()
        )));
    }
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

    /// A bundle file named like a gzip sidecar would be staged onto the same
    /// path as the sidecar `build.rs` generates for its non-prefixed sibling,
    /// so the binary would serve one asset's bytes under the other's name. The
    /// bundle is refused here because nothing downstream can tell: the catalog
    /// compiles, and the console fails to boot at runtime instead.
    #[test]
    fn a_bundle_claiming_the_gzip_sidecar_namespace_is_refused() {
        let dir = bundle(&[
            ("index.html", BASE_PATH_SENTINEL),
            (
                "gzip__app.wasm",
                "not a sidecar, a bundle file that squats the name",
            ),
            ("app.wasm", "module"),
            ("app.js", "j"),
            ("app.css", "c"),
        ]);

        let error = check(dir.path()).expect_err("a squatted sidecar name");
        assert!(error.to_string().contains(GZIP_PREFIX), "{error}");
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

    /// Architecture/toolchain lint: the CLI the image installs must be the
    /// framework the UI crate pins. This reads repository source policy and is
    /// not functional application coverage.
    #[test]
    fn architecture_lint_matches_the_image_cli_to_the_ui_crate_pin() {
        check_cli_pin().expect("the Dioxus CLI pin must equal the crate's dioxus requirement");
    }

    /// Checker-fixture lint: the parsers cannot pass by finding nothing. A
    /// `Dockerfile` with no `ARG` and a manifest with no `dioxus` requirement
    /// must both read as absent rather than as a silent match.
    #[test]
    fn the_pin_is_read_from_both_files_and_absent_reads_as_absent() {
        let dockerfile = "ARG DIOXUS_CLI_VERSION=0.7.10\nRUN cargo install dioxus-cli\n";
        assert_eq!(dioxus_cli_arg(dockerfile).as_deref(), Some("0.7.10"));
        assert_eq!(dioxus_cli_arg("ARG WASM_TARGET=wasm32"), None);
        assert_eq!(dioxus_cli_arg("# ARG DIOXUS_CLI_VERSION=0.7.10"), None);

        // An exact requirement carries the `=`; a caret one does not.
        let manifest = "[dependencies]\ndioxus = { version = \"=0.7.10\", features = [\"web\"] }\n";
        assert_eq!(dioxus_requirement(manifest).as_deref(), Some("0.7.10"));
        let relaxed = "[dependencies]\ndioxus = { version = \"0.8.0\" }\n";
        assert_eq!(dioxus_requirement(relaxed).as_deref(), Some("0.8.0"));
        assert_eq!(dioxus_requirement("[dependencies]\nserde = \"1\""), None);
    }

    /// The real workspace: the profile `dx bundle` builds the console with must
    /// not ask for `--strip-all`, which strips wasm-bindgen's own metadata.
    #[test]
    fn the_web_profile_does_not_strip_wasm_bindgen_metadata() {
        check_web_strip_profile()
            .expect("the console's web profile must keep the wasm-bindgen sections");
    }

    /// The parser, and the value it must refuse. `strip = true` is the spelling
    /// that becomes `--strip-all`; `"debuginfo"` is the one that is safe. The
    /// other two cases matter because a parser that finds nothing must not read
    /// as safe — a missing profile is exactly what makes dx fall back to
    /// `[profile.release]`, where `strip = true` lives.
    #[test]
    fn the_web_profile_strip_is_read_from_the_right_section() {
        let manifest = "[profile.release]\nstrip = true\n\n[profile.wasm-release]\n\
                        inherits = \"release\"\nstrip = \"debuginfo\"\n";
        assert_eq!(web_profile_strip(manifest).as_deref(), Some("debuginfo"));

        // The value the check refuses, in both spellings cargo accepts.
        let symbols = "[profile.wasm-release]\nstrip = true\n";
        assert_eq!(web_profile_strip(symbols).as_deref(), Some("true"));
        assert!(check_web_strip_profile_for(symbols).is_err());

        // Absent reads as absent, not as a silent pass.
        assert_eq!(web_profile_strip("[profile.release]\nstrip = true\n"), None);
        assert!(check_web_strip_profile_for("[profile.release]\nstrip = true\n").is_err());

        // A commented-out setting is not a setting.
        let commented = "[profile.wasm-release]\n# strip = true\n";
        assert_eq!(web_profile_strip(commented), None);
        assert!(check_web_strip_profile_for(commented).is_err());

        // A nested table does not truncate the profile above it, and the first
        // `strip` in the body is the profile's own.
        let nested = "[profile.wasm-release]\nstrip = \"debuginfo\"\n\
                      [profile.release.package.\"*\"]\nstrip = true\n";
        assert_eq!(web_profile_strip(nested).as_deref(), Some("debuginfo"));
    }
}
