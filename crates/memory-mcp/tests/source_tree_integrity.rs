//! Every Rust source file under `src/` must be reachable from a production
//! Cargo root through Rust file-module declarations.
//!
//! rustc only compiles files reached from a crate root. Counting incoming
//! declarations from every file on disk is not enough: an unreachable cycle can
//! give its members incoming edges while remaining invisible to rustc. Cargo's
//! lib/bin targets are the roots; this test walks outward from those roots and
//! compares the result to the source inventory. The walk deliberately unions
//! feature-gated declarations, including declarations behind disabled features.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

#[path = "support/rust_source.rs"]
mod rust_source;

use rust_source::{Token, is_identifier, is_open_group, matching_group, tokenize};

#[test]
fn every_source_file_is_reachable_from_a_cargo_root() {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = manifest_dir.join("src");
    let files = source_files(&src);
    assert!(
        !files.is_empty(),
        "found no source files under {}, so the guard would pass on an empty tree",
        src.display()
    );

    let roots = cargo_root_files(manifest_dir);
    let (reachable, diagnostics) = rooted_module_graph(&src, &roots);
    assert!(
        diagnostics.is_empty(),
        "the source graph contains declarations the bounded walker cannot resolve:\n{diagnostics:#?}"
    );

    let orphans: Vec<&PathBuf> = files
        .iter()
        .filter(|file| !reachable.contains(*file))
        .collect();
    assert!(
        orphans.is_empty(),
        "rustc cannot reach {} checked-in source file(s) under {} from any Cargo lib/bin root:\n{orphans:#?}",
        orphans.len(),
        src.display(),
    );
}

/// Keep path attributes out of the production source graph until their Rust
/// resolution rules are implemented here. Silently guessing is not a valid
/// fallback: a wrong path can make a real file look like an orphan or let an
/// unrelated file make the guard pass.
#[test]
fn no_source_file_uses_a_path_attribute() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let offenders: Vec<String> = source_files(&src)
        .into_iter()
        .filter(|file| {
            fs::read_to_string(file)
                .map(|text| {
                    let tokens = tokenize(&text).unwrap_or_default();
                    has_path_attribute(&tokens)
                })
                .unwrap_or(false)
        })
        .map(|file| file.display().to_string())
        .collect();

    assert!(
        offenders.is_empty(),
        "`#[path]` is not supported by the rooted source walker. Teach the walker \
         to resolve it or keep the construct prohibited:\n{offenders:#?}"
    );
}

fn cargo_root_files(manifest_dir: &Path) -> Vec<PathBuf> {
    let cargo = std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into());
    let output = Command::new(cargo)
        .args([
            "metadata",
            "--format-version",
            "1",
            "--no-deps",
            "--locked",
            "--offline",
        ])
        .current_dir(manifest_dir)
        .output()
        .unwrap_or_else(|error| panic!("could not run cargo metadata: {error}"));
    assert!(
        output.status.success(),
        "cargo metadata failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let metadata: serde_json::Value = serde_json::from_slice(&output.stdout)
        .unwrap_or_else(|error| panic!("cargo metadata returned invalid JSON: {error}"));
    roots_from_metadata(&metadata, &manifest_dir.join("Cargo.toml"))
}

/// Discover only lib/bin roots for this package. Integration-test files do not
/// establish that a production source file is reachable.
fn roots_from_metadata(metadata: &serde_json::Value, manifest: &Path) -> Vec<PathBuf> {
    let wanted = manifest.to_string_lossy();
    let Some(packages) = metadata
        .get("packages")
        .and_then(serde_json::Value::as_array)
    else {
        return Vec::new();
    };
    let Some(package) = packages.iter().find(|package| {
        package
            .get("manifest_path")
            .and_then(serde_json::Value::as_str)
            == Some(wanted.as_ref())
    }) else {
        return Vec::new();
    };
    let Some(targets) = package.get("targets").and_then(serde_json::Value::as_array) else {
        return Vec::new();
    };

    let mut roots = Vec::new();
    for target in targets {
        let is_production_root = target
            .get("kind")
            .and_then(serde_json::Value::as_array)
            .is_some_and(|kinds| {
                kinds
                    .iter()
                    .any(|kind| matches!(kind.as_str(), Some("lib" | "bin")))
            });
        if !is_production_root {
            continue;
        }
        if let Some(path) = target.get("src_path").and_then(serde_json::Value::as_str) {
            roots.push(PathBuf::from(path));
        }
    }
    roots.sort();
    roots.dedup();
    roots
}

fn source_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = fs::read_dir(&current)
            .unwrap_or_else(|error| panic!("{} is not readable: {error}", current.display()));
        for entry in entries {
            let entry = entry.unwrap_or_else(|error| {
                panic!("unreadable entry in {}: {error}", current.display())
            });
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// Detect `path = ...` inside an attribute, including nested `cfg_attr(...,
/// path = ...)`. Literal contents have already been removed by the lexer, so a
/// string that discusses the attribute cannot trip this check.
fn has_path_attribute(tokens: &[Token]) -> bool {
    let mut index = 0;
    while index + 1 < tokens.len() {
        if tokens[index].text == "#"
            && tokens[index + 1].text == "["
            && let Some(close) = matching_group(tokens, index + 1)
        {
            if tokens[index + 2..close]
                .windows(2)
                .any(|pair| pair[0].text == "path" && pair[1].text == "=")
            {
                return true;
            }
            index = close + 1;
        } else {
            index += 1;
        }
    }
    false
}

fn rooted_module_graph(src: &Path, roots: &[PathBuf]) -> (BTreeSet<PathBuf>, Vec<String>) {
    let canonical_src = src
        .canonicalize()
        .unwrap_or_else(|error| panic!("{} is not readable: {error}", src.display()));
    let mut graph = ModuleGraph {
        src: &canonical_src,
        reachable: BTreeSet::new(),
        visited: BTreeSet::new(),
        diagnostics: Vec::new(),
    };
    for root in roots {
        let canonical_root = match root.canonicalize() {
            Ok(path) => path,
            Err(error) => {
                graph.diagnostics.push(format!(
                    "Cargo root {} does not resolve: {error}",
                    root.display()
                ));
                continue;
            }
        };
        if !canonical_root.starts_with(&canonical_src) {
            graph.diagnostics.push(format!(
                "Cargo root {} is outside {}",
                canonical_root.display(),
                canonical_src.display()
            ));
            continue;
        }
        graph.walk_file(&canonical_root, true);
    }
    (graph.reachable, graph.diagnostics)
}

struct ModuleGraph<'a> {
    src: &'a Path,
    reachable: BTreeSet<PathBuf>,
    visited: BTreeSet<PathBuf>,
    diagnostics: Vec<String>,
}

impl ModuleGraph<'_> {
    fn walk_file(&mut self, file: &Path, is_root: bool) {
        let canonical = match file.canonicalize() {
            Ok(path) if path.starts_with(self.src) => path,
            Ok(path) => {
                self.diagnostics.push(format!(
                    "module path {} escapes src/ to {}",
                    file.display(),
                    path.display()
                ));
                return;
            }
            Err(error) => {
                self.diagnostics.push(format!(
                    "module path {} does not resolve: {error}",
                    file.display()
                ));
                return;
            }
        };
        if !self.visited.insert(canonical.clone()) {
            return;
        }
        self.reachable.insert(canonical.clone());
        let source = match fs::read_to_string(&canonical) {
            Ok(source) => source,
            Err(error) => {
                self.diagnostics
                    .push(format!("{} is not readable: {error}", canonical.display()));
                return;
            }
        };
        let tokens = match tokenize(&source) {
            Ok(tokens) => tokens,
            Err(error) => {
                self.diagnostics
                    .push(format!("{}: {error}", canonical.display()));
                return;
            }
        };
        let search_dir = module_search_dir(&canonical, is_root);
        self.walk_scope(&canonical, &tokens, &search_dir);
    }

    fn walk_scope(&mut self, source_file: &Path, tokens: &[Token], search_dir: &Path) {
        if has_path_attribute(tokens) {
            self.diagnostics.push(format!(
                "{}: #[path] module declarations are prohibited until resolved",
                source_file.display()
            ));
            return;
        }
        let mut index = 0;
        while index < tokens.len() {
            if tokens[index].text == "macro_rules"
                && tokens.get(index + 1).is_some_and(|token| token.text == "!")
            {
                let Some(open) = (index + 2..tokens.len()).find(|&at| tokens[at].text == "{")
                else {
                    self.diagnostics.push(format!(
                        "{}: unsupported macro_rules declaration",
                        source_file.display()
                    ));
                    return;
                };
                let Some(close) = matching_group(tokens, open) else {
                    self.diagnostics.push(format!(
                        "{}: unterminated macro_rules body",
                        source_file.display()
                    ));
                    return;
                };
                index = close + 1;
                continue;
            }

            if tokens[index].text == "include"
                && tokens.get(index + 1).is_some_and(|token| token.text == "!")
            {
                if source_file == self.src.join("ui/assets.rs") {
                    index += 2;
                    continue;
                }
                self.diagnostics.push(format!(
                    "{}: include! may declare modules the rooted walker cannot inspect",
                    source_file.display()
                ));
                return;
            }

            if tokens[index].text == "!"
                && index > 0
                && tokens
                    .get(index + 1)
                    .is_some_and(|token| is_open_group(&token.text))
            {
                if let Some(close) = matching_group(tokens, index + 1) {
                    let body = &tokens[index + 2..close];
                    if body.windows(3).any(|window| {
                        window[0].text == "mod"
                            && is_identifier(&window[1].text)
                            && matches!(window[2].text.as_str(), ";" | "{")
                    }) {
                        self.diagnostics.push(format!(
                            "{}: macro invocation contains a module declaration that this \
                             lexical guard cannot resolve",
                            source_file.display()
                        ));
                        return;
                    }
                    index = close + 1;
                    continue;
                }
                self.diagnostics.push(format!(
                    "{}: unterminated macro invocation",
                    source_file.display()
                ));
                return;
            }

            if tokens[index].text == "mod"
                && let Some(name) = tokens.get(index + 1)
                && is_identifier(&name.text)
            {
                match tokens.get(index + 2).map(|token| token.text.as_str()) {
                    Some(";") => {
                        self.follow_external(source_file, search_dir, &name.text);
                        index += 3;
                        continue;
                    }
                    Some("{") => {
                        if let Some(close) = matching_group(tokens, index + 2) {
                            self.walk_scope(
                                source_file,
                                &tokens[index + 3..close],
                                &search_dir.join(&name.text),
                            );
                            index = close + 1;
                            continue;
                        }
                        self.diagnostics.push(format!(
                            "{}: unterminated inline module {}",
                            source_file.display(),
                            name.text
                        ));
                        return;
                    }
                    _ => {}
                }
            }
            index += 1;
        }
    }

    fn follow_external(&mut self, source_file: &Path, search_dir: &Path, name: &str) {
        let flat = search_dir.join(format!("{name}.rs"));
        let nested = search_dir.join(name).join("mod.rs");
        let flat_exists = flat.is_file();
        let nested_exists = nested.is_file();
        match (flat_exists, nested_exists) {
            (true, false) => self.walk_file(&flat, false),
            (false, true) => self.walk_file(&nested, false),
            (false, false) => self.diagnostics.push(format!(
                "{}: mod {name}; has no file at {} or {}",
                source_file.display(),
                flat.display(),
                nested.display()
            )),
            (true, true) => self.diagnostics.push(format!(
                "{}: mod {name}; is ambiguous; both {} and {} exist",
                source_file.display(),
                flat.display(),
                nested.display()
            )),
        }
    }
}

fn module_search_dir(file: &Path, is_root: bool) -> PathBuf {
    let parent = file.parent().unwrap_or_else(|| Path::new(""));
    if is_root || file.file_name().is_some_and(|name| name == "mod.rs") {
        parent.to_path_buf()
    } else {
        parent.join(file.file_stem().unwrap_or_default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn fixture() -> (TempDir, PathBuf) {
        let temp = TempDir::new().expect("fixture dir");
        let src = temp.path().join("src");
        fs::create_dir_all(&src).expect("src dir");
        let canonical = src.canonicalize().expect("canonical src");
        (temp, canonical)
    }

    fn write(src: &Path, relative: &str, text: &str) -> PathBuf {
        let path = src.join(relative);
        fs::create_dir_all(path.parent().expect("parent")).expect("parent dir");
        fs::write(&path, text).expect("write fixture");
        path
    }

    #[test]
    fn unrooted_mod_cycle_is_reported_unreachable() {
        let (_temp, src) = fixture();
        let root = write(&src, "lib.rs", "");
        write(&src, "a.rs", "mod b;\n");
        write(&src, "b.rs", "mod a;\n");
        let (reachable, diagnostics) = rooted_module_graph(&src, &[root]);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        assert_eq!(reachable.len(), 1);
        assert!(!reachable.contains(&src.join("a.rs")));
        assert!(!reachable.contains(&src.join("b.rs")));
    }

    #[test]
    fn ordinary_module_files_have_no_sibling_fallback() {
        let (_temp, src) = fixture();
        let root = write(&src, "lib.rs", "mod a;\n");
        write(&src, "a.rs", "");
        write(&src, "b.rs", "");
        let (reachable, diagnostics) = rooted_module_graph(&src, &[root]);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        assert!(reachable.contains(&src.join("a.rs")));
        assert!(!reachable.contains(&src.join("b.rs")));
    }

    #[test]
    fn inline_modules_resolve_children_under_the_inline_directory() {
        let (_temp, src) = fixture();
        let root = write(&src, "lib.rs", "mod outer { mod inner; }\n");
        write(&src, "outer/inner.rs", "");
        let (reachable, diagnostics) = rooted_module_graph(&src, &[root]);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        assert!(reachable.contains(&src.join("outer/inner.rs")));
    }

    #[test]
    fn feature_gated_declarations_are_in_the_union_graph() {
        let (_temp, src) = fixture();
        let root = write(
            &src,
            "lib.rs",
            "#[cfg(feature = \"optional\")] mod optional;\n",
        );
        write(&src, "optional.rs", "");
        let (reachable, diagnostics) = rooted_module_graph(&src, &[root]);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        assert!(reachable.contains(&src.join("optional.rs")));
    }

    #[test]
    fn module_text_in_comments_strings_and_macros_is_not_a_declaration() {
        let (_temp, src) = fixture();
        let root = write(
            &src,
            "lib.rs",
            r#"
                // mod comment_only;
                const TEXT: &str = "mod string_only;";
                macro_rules! declare_later { () => { mod macro_only; } }
            "#,
        );
        let (reachable, diagnostics) = rooted_module_graph(&src, &[root]);
        assert!(diagnostics.is_empty(), "{diagnostics:#?}");
        assert_eq!(reachable.len(), 1);
    }

    #[test]
    fn macro_generated_module_declarations_fail_closed() {
        let (_temp, src) = fixture();
        let root = write(&src, "lib.rs", "declare! { mod generated; }\n");

        let (_, diagnostics) = rooted_module_graph(&src, &[root]);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert!(
            diagnostics[0].contains("macro invocation"),
            "{diagnostics:#?}"
        );
    }

    #[test]
    fn missing_and_ambiguous_module_files_fail_closed() {
        let (_temp, src) = fixture();
        let root = write(&src, "lib.rs", "mod missing;\n");
        let (_, diagnostics) = rooted_module_graph(&src, std::slice::from_ref(&root));
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");

        write(&src, "missing.rs", "");
        write(&src, "missing/mod.rs", "");
        let (_, diagnostics) = rooted_module_graph(&src, &[root]);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert!(diagnostics[0].contains("ambiguous"), "{diagnostics:#?}");
    }

    #[test]
    fn path_attributes_inside_cfg_attr_fail_closed() {
        let (_temp, src) = fixture();
        let root = write(
            &src,
            "lib.rs",
            "#[cfg_attr(feature = \"unused\", path = \"other.rs\")] mod selected;\n",
        );
        write(&src, "selected.rs", "");
        write(&src, "other.rs", "");

        let (_, diagnostics) = rooted_module_graph(&src, &[root]);
        assert_eq!(diagnostics.len(), 1, "{diagnostics:#?}");
        assert!(diagnostics[0].contains("#[path]"), "{diagnostics:#?}");
    }

    #[test]
    fn cargo_metadata_selects_only_lib_and_bin_roots() {
        let manifest = Path::new("/workspace/crates/memory-mcp/Cargo.toml");
        let metadata = serde_json::json!({
            "packages": [{
                "manifest_path": manifest,
                "targets": [
                    {"kind": ["lib"], "src_path": "/workspace/crates/memory-mcp/src/lib.rs"},
                    {"kind": ["bin"], "src_path": "/workspace/crates/memory-mcp/src/bin/server.rs"},
                    {"kind": ["test"], "src_path": "/workspace/crates/memory-mcp/tests/check.rs"},
                    {"kind": ["custom-build"], "src_path": "/workspace/crates/memory-mcp/build.rs"}
                ]
            }]
        });
        let roots = roots_from_metadata(&metadata, manifest);
        assert_eq!(
            roots,
            vec![
                PathBuf::from("/workspace/crates/memory-mcp/src/bin/server.rs"),
                PathBuf::from("/workspace/crates/memory-mcp/src/lib.rs")
            ]
        );
    }
}
