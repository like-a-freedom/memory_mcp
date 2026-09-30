//! Every `pub` helper in `tests/common/mod.rs` must have a caller.
//!
//! `tests/common/mod.rs` is compiled into each test binary separately, so
//! rustc cannot see a helper as dead when no *that* binary uses it — the
//! unused warning that would catch it does not fire, and a helper with one
//! caller among forty test files is a live function to the compiler and dead
//! weight to the reader. Suppressing it with `#[allow(dead_code)]` is the
//! shape the problem takes, and it is invisible.
//!
//! So this test reads the file and searches the tests that use it. The
//! baseline is deliberately not encoded: a new helper with no caller is
//! reported by name, and the fix is to write the test or delete the helper.

use std::fs;
use std::path::{Path, PathBuf};

fn common_module() -> (PathBuf, String) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/common/mod.rs");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} is not readable: {e}", path.display()));
    (path, text)
}

/// The names of every `pub async fn` / `pub fn` in `text`, including those
/// inside a `#[cfg(...)]` block. A multi-line signature is joined first, so a
/// helper whose `(` sits on the next line is still found.
fn helper_names(text: &str) -> Vec<String> {
    let mut joined = String::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        joined.push_str(trimmed);
        if !trimmed.ends_with('{') && !trimmed.ends_with(';') && !trimmed.ends_with(',') {
            joined.push(' ');
        }
    }

    let mut names = Vec::new();
    let mut i = 0;
    while let Some(found) = joined[i..]
        .find("pub fn ")
        .into_iter()
        .chain(joined[i..].find("pub async fn "))
        .min()
    {
        let start = i + found;
        let after = start + joined[start..].find("fn ").expect("found the fn keyword") + 3;
        let end = joined[after..]
            .find(|c: char| !(c.is_alphanumeric() || c == '_'))
            .map(|n| after + n)
            .unwrap_or(joined.len());
        let name = &joined[after..end];
        if !name.is_empty() {
            names.push(name.to_string());
        }
        i = end.min(joined.len());
    }
    names
}

#[test]
fn every_common_helper_has_a_caller() {
    let (common_path, common_text) = common_module();
    let tests_dir = common_path
        .parent()
        .and_then(Path::parent)
        .expect("tests/common/mod.rs has two ancestors");

    let mut sources = String::new();
    let mut entries: Vec<PathBuf> = fs::read_dir(tests_dir)
        .expect("tests/ is readable")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "rs"))
        .collect();
    entries.sort();
    for entry in &entries {
        if let Ok(text) = fs::read_to_string(entry) {
            sources.push_str(&text);
        }
    }
    assert!(
        !sources.is_empty(),
        "no test sources were read from {}, so this test would pass on an empty set",
        tests_dir.display()
    );

    let helpers = helper_names(&common_text);
    let uncalled: Vec<&str> = helpers
        .iter()
        .map(String::as_str)
        .filter(|name| {
            // A caller names the helper as `common::name` or `mod::name`. The
            // check is on whole identifiers, not substrings: a plain
            // `contains` scores `make_service_with_query_logging` as called
            // by the definition of `make_service`, which is the exact false
            // pass this test exists to prevent — the helper really does have
            // no caller outside this file. A glob import would defeat the
            // check, and none of these tests uses one.
            !sources.contains(&format!("::{name}"))
        })
        .collect();

    assert!(
        uncalled.is_empty(),
        "{} helper(s) in tests/common/mod.rs have no caller in any test file. \
         A helper nothing calls is not a seam, it is weight — and because \
         `common/mod.rs` is compiled per test binary, the compiler cannot \
         report it. Write the test that needs it, or delete it:\n\n{:#?}",
        uncalled.len(),
        uncalled
    );
}
