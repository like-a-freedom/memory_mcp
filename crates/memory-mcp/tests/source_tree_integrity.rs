//! Every source file in this crate must be reachable from a `mod`
//! declaration.
//!
//! rustc only compiles what the module graph reaches. A file that no `mod`
//! names is invisible to the compiler: it can hold broken code, a stale
//! duplicate of a moved function, or a `todo!()`, and every check the project
//! runs will still pass green. ADR-0061 recorded that the compiler graph is
//! the source of truth and ADR-0063 retired the check that used to assert it;
//! ADR-0065 reinstates it, because a fact nothing derives is a fact that
//! decays.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[test]
fn every_source_file_is_reachable_from_a_mod_declaration() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");

    let files = source_files(&src);
    assert!(
        !files.is_empty(),
        "found no source files under {}, so the guard would pass on an empty tree",
        src.display()
    );

    let mut declared: BTreeSet<PathBuf> = BTreeSet::new();
    for file in &files {
        let text = fs::read_to_string(file)
            .unwrap_or_else(|e| panic!("{} is not readable: {e}", file.display()));
        for name in module_declarations(&text) {
            declared.extend(resolve(file, &name));
        }
    }
    // A crate root and a binary root are named by Cargo, not by a `mod`.
    declared.extend(cargo_root_files());

    let orphans: Vec<&PathBuf> = files.iter().filter(|f| !declared.contains(*f)).collect();
    assert!(
        orphans.is_empty(),
        "rustc does not compile {} file(s) under {}, because no `mod` declaration \
         names them. Delete them, or declare them:\n{:#?}",
        orphans.len(),
        src.display(),
        orphans
    );
}

/// The crate uses no `#[path]` module declarations, so the resolver above
/// does not have to understand them. This assertion is what keeps that true:
/// without it, the first `#[path = "…"] mod x;` would make its file look like
/// an orphan, and the fix would be to weaken this test rather than teach the
/// resolver about a form the crate does not use.
#[test]
fn no_source_file_uses_a_path_attribute() {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let offenders: Vec<String> = source_files(&src)
        .iter()
        .filter(|file| {
            fs::read_to_string(file)
                .map(|text| strip_comments(&text).contains("#[path"))
                .unwrap_or(false)
        })
        .map(|file| file.display().to_string())
        .collect();

    assert!(
        offenders.is_empty(),
        "`#[path]` is not supported by the reachability guard. Either teach it \
         to resolve the attribute, or teach this test about it — do not leave \
         the two disagreeing:\n{offenders:#?}"
    );
}

/// The crate roots Cargo compiles, which no `mod` declaration names: the
/// library root, the implicit `src/main.rs` binary, and every `[[bin]]` and
/// `[[example]]` path.
///
/// These are read out of `Cargo.toml` rather than hardcoded, because the
/// alternative is a guard with its own blind spot: a new binary target would
/// be added to the manifest, compiled by CI, and reported here as an orphan.
fn cargo_root_files() -> Vec<PathBuf> {
    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let manifest = fs::read_to_string(manifest_dir.join("Cargo.toml"))
        .unwrap_or_else(|e| panic!("Cargo.toml is not readable: {e}"));
    let mut roots = vec![manifest_dir.join("src/lib.rs")];
    // An implicit `src/main.rs` is a binary root when it exists, and nothing
    // at all when it does not — a library-only crate has no such target.
    let implicit_bin = manifest_dir.join("src/main.rs");
    if implicit_bin.is_file() {
        roots.push(implicit_bin);
    }

    let target_paths = |table: &str| -> Vec<String> {
        let mut paths = Vec::new();
        let mut in_table = false;
        for line in manifest.lines() {
            let line = line.trim();
            if line.starts_with('[') {
                in_table = line == format!("[{table}]") || line == format!("[[{table}]]");
                continue;
            }
            if in_table
                && let Some(path) = line.strip_prefix("path")
                && let Some(value) = path.trim_start().strip_prefix('=')
            {
                paths.push(value.trim().trim_matches('"').to_string());
            }
        }
        paths
    };
    for table in ["bin", "example", "bench", "test"] {
        for path in target_paths(table) {
            roots.push(manifest_dir.join(path));
        }
    }
    roots
}

fn source_files(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut stack = vec![dir.to_path_buf()];
    while let Some(current) = stack.pop() {
        let entries = fs::read_dir(&current)
            .unwrap_or_else(|e| panic!("{} is not readable: {e}", current.display()));
        for entry in entries {
            let entry =
                entry.unwrap_or_else(|e| panic!("unreadable entry in {}: {e}", current.display()));
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                found.push(path);
            }
        }
    }
    found.sort();
    found
}

/// The names of every `mod x;` declaration in `text`.
///
/// `mod x { … }` is an inline module and names no file, so it is skipped —
/// the crate has 266 of them. Visibility keywords and attributes on the
/// preceding line (`#[cfg(feature = "…")] pub mod x;`) do not change the name.
fn module_declarations(text: &str) -> Vec<String> {
    let code = strip_comments(text);
    let mut names = Vec::new();
    for line in code.lines() {
        let line = line.trim();
        if line.contains('{') {
            continue;
        }
        let Some(rest) = split_mod_keyword(line) else {
            continue;
        };
        let rest = rest.trim();
        let Some(name) = rest.strip_suffix(';').map(str::trim) else {
            continue;
        };
        if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
            names.push(name.to_string());
        }
    }
    names
}

/// The text after the `mod` keyword on `line`, or `None` when the line does not
/// declare a module. This is deliberately not a regex: a regex over `mod`
/// would also match `model`, and the crate is full of words starting with it.
fn split_mod_keyword(line: &str) -> Option<&str> {
    let mut search = line;
    loop {
        let found = search.find("mod")?;
        let before_ok = search[..found]
            .chars()
            .next_back()
            .is_none_or(|c| c.is_whitespace());
        let after = &search[found + 3..];
        let after_ok = after
            .chars()
            .next()
            .is_some_and(|c| c.is_whitespace() || c == '_');
        if before_ok && after_ok {
            return Some(after);
        }
        search = after;
    }
}

/// Blank out comments so a `mod` mentioned in prose is not read as a
/// declaration. String literals are left alone: no string in this crate
/// contains a `mod x;` declaration, and a false positive there would be a
/// silent one, which is the failure mode this test exists to prevent.
fn strip_comments(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars().peekable();
    let mut in_block = false;
    while let Some(c) = chars.next() {
        if in_block {
            if c == '*' && chars.peek() == Some(&'/') {
                chars.next();
                in_block = false;
            }
            continue;
        }
        match c {
            '/' if chars.peek() == Some(&'/') => {
                for c in chars.by_ref() {
                    if c == '\n' {
                        out.push('\n');
                        break;
                    }
                }
            }
            '/' if chars.peek() == Some(&'*') => {
                chars.next();
                in_block = true;
            }
            _ => out.push(c),
        }
    }
    out
}

/// Where a `mod <name>;` in `file` could name a file on disk.
///
/// Rust 2018 gives a module one home plus a legacy fallback. A module file
/// `src/tools.rs` declares `pub mod assemble_context;`, which resolves to
/// `src/tools/assemble_context.rs` — and `src/tools.rs` is only a *sibling*
/// file, so the directory `src/tools/` has to exist for that to be the
/// reading. The fallback path `src/assemble_context.rs` is returned too,
/// because Rust accepts it and a caller relying on it must not be reported as
/// an orphan.
fn resolve(file: &Path, name: &str) -> Vec<PathBuf> {
    let Some(dir) = file.parent() else {
        return Vec::new();
    };
    let stem = file
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or_default();
    // A crate root's children live beside it, not in a directory named after
    // the file: `src/lib.rs` declares `pub mod tools;` for `src/tools.rs`.
    let is_crate_root = stem == "lib" || stem == "main";
    let module_dir = if is_crate_root || stem == "mod" {
        dir.to_path_buf()
    } else {
        let candidate = dir.join(stem);
        if candidate.is_dir() {
            candidate
        } else {
            dir.to_path_buf()
        }
    };
    vec![
        module_dir.join(format!("{name}.rs")),
        module_dir.join(name).join("mod.rs"),
    ]
}
