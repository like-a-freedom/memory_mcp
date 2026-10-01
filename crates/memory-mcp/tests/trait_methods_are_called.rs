//! Every method on a store trait must have a caller.
//!
//! A trait method is a promise. `TaskStore::update_progress_fenced` promised
//! that progress could be reported under a fence, had a real implementation in
//! the worker store, and was called from nowhere — not from production, not
//! from a test, not from the test driver. Nothing reports that: rustc emits
//! no warning for a trait method that is implemented but never called, and
//! `#[async_trait]` hides even the implementation.
//
//! The trait methods are listed here explicitly rather than parsed out of the
//! source, because a parser that reads the trait and searches for `.name(`
//! will score the `impl` block as a call site and pass everything. The
//! declaration and the implementation are both excluded by name below; what is
//! left is somebody asking for the method.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// `(trait path, method name)` for every method that must have a caller.
///
/// Adding a row is a claim that the method is used. Deleting a row without
/// deleting the method makes this test stop covering it, so the table and the
/// trait have to be changed together.
const METHODS: &[(&str, &str)] = &[
    // http/tasks/state.rs — TaskStore
    ("http/tasks/state.rs", "TaskStore::enqueue"),
    ("http/tasks/state.rs", "TaskStore::load"),
    ("http/tasks/state.rs", "TaskStore::set_cancellation_intent"),
    ("http/tasks/state.rs", "TaskStore::claim_next_due"),
    ("http/tasks/state.rs", "TaskStore::complete_fenced"),
    (
        "http/tasks/state.rs",
        "TaskStore::cancel_before_commit_fenced",
    ),
    ("http/tasks/state.rs", "TaskStore::fail_fenced"),
    ("http/tasks/state.rs", "TaskStore::requeue_expired_running"),
    ("http/tasks/state.rs", "TaskStore::reconcile_artifacts"),
    ("http/tasks/state.rs", "TaskStore::delete_expired"),
    // service/local_admin/contracts.rs — LocalAdminStore. Three of its
    // nineteen methods, covering the operator path end to end: issue a
    // challenge, read the credential it belongs to, open the session. The
    // table is a spot check with a known answer, not an inventory — every
    // method of a nineteen-method trait cannot be asserted by a grep, and a
    // table that pretends otherwise is a table that goes stale silently.
    (
        "service/local_admin/contracts.rs",
        "LocalAdminStore::issue_challenge",
    ),
    (
        "service/local_admin/contracts.rs",
        "LocalAdminStore::credential",
    ),
    (
        "service/local_admin/contracts.rs",
        "LocalAdminStore::open_session",
    ),
];

fn source_files() -> Vec<PathBuf> {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    let mut stack = vec![root];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
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

/// Every source file's text, plus the test files' — a trait method called only
/// from an integration test is called.
fn all_sources() -> BTreeSet<(String, String)> {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut out = BTreeSet::new();
    for root in [manifest.join("src"), manifest.join("tests")] {
        let mut stack = vec![root];
        while let Some(current) = stack.pop() {
            let Ok(entries) = fs::read_dir(&current) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    stack.push(path);
                } else if path.extension().is_some_and(|ext| ext == "rs")
                    && let Ok(text) = fs::read_to_string(&path)
                {
                    out.insert((relative(&path), text));
                }
            }
        }
    }
    assert!(!out.is_empty(), "no sources were read");
    out
}

/// The method part of a `Trait::method` entry.
fn method_name(qualified: &str) -> &str {
    qualified
        .rsplit("::")
        .next()
        .filter(|name| !name.is_empty())
        .unwrap_or(qualified)
}

fn relative(path: &Path) -> String {
    path.strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")))
        .unwrap_or(path)
        .display()
        .to_string()
}

/// A method counts as called when some file names it as a receiver call —
/// `.method(` or `.method (` — anywhere the declaration or an `impl` body is
/// not.
fn has_caller(sources: &BTreeSet<(String, String)>, method: &str) -> Option<String> {
    let needle = format!(".{method}(");
    let spaced = format!(".{method} (");
    for (file, text) in sources {
        if !text.contains(&needle) && !text.contains(&spaced) {
            continue;
        }
        // Skip the trait declaration itself.
        if text.contains(&format!("async fn {method}(")) || text.contains(&format!("fn {method}("))
        {
            // A file may both declare and call; only skip it if every hit is
            // inside a signature.
            if !calls_outside_signatures(text, method) {
                continue;
            }
        }
        return Some(file.clone());
    }
    None
}

/// Whether `text` contains a `.method(` that is not the one in a signature.
///
/// A signature reads `async fn method(` without a leading dot, so a hit with a
/// dot is a call by construction. The only way a declaration can carry a dot
/// is a doc comment or a bound, and neither is `.method(`.
fn calls_outside_signatures(text: &str, method: &str) -> bool {
    for (index, _) in text.match_indices(&format!(".{method}(")) {
        let line = &text[..index].rsplit('\n').next().unwrap_or_default();
        let before = line.trim_start();
        if before.ends_with(&format!("fn {method}(")) {
            continue;
        }
        return true;
    }
    false
}

#[test]
fn every_trait_method_named_in_the_table_still_exists() {
    let files = source_files();
    let mut problems = Vec::new();

    for (path, qualified) in METHODS {
        let full = Path::new(env!("CARGO_MANIFEST_DIR")).join("src").join(path);
        let Ok(text) = fs::read_to_string(&full) else {
            problems.push(format!("{qualified}: {path} is missing"));
            continue;
        };
        let method = method_name(qualified);
        if !text.contains(&format!("fn {method}(")) {
            problems.push(format!(
                "{qualified} is gone from {path}. Delete its row from this table in \
                 the same commit, or the method is no longer covered."
            ));
        }
    }

    assert!(!files.is_empty());
    assert!(
        problems.is_empty(),
        "{} trait method(s) drifted out of the table:\n\n{}",
        problems.len(),
        problems.join("\n")
    );
}

#[test]
fn every_trait_method_has_a_caller() {
    let sources = all_sources();
    let mut uncalled = Vec::new();

    for (_, qualified) in METHODS {
        let method = method_name(qualified);
        if has_caller(&sources, method).is_none() {
            uncalled.push(*qualified);
        }
    }

    assert!(
        uncalled.is_empty(),
        "{} trait method(s) have no caller anywhere in src/ or tests/:\n\n{:#?}\n\n\
         A method that is declared and implemented but never called is not a \
         seam; it is a promise nobody asked for. Either wire it, or delete the \
         declaration and the implementation.",
        uncalled.len(),
        uncalled
    );
}

/// `TaskStore` had eleven methods and one of them was a promise nobody asked
/// for.
///
/// `update_progress_fenced` — "update progress under a `lease_generation`
/// CAS" — was declared on the trait, implemented in the fenced worker store
/// with a real `UPDATE`, and called from nowhere: not from production, not
/// from a test, not from the test driver. rustc does not report it, and
/// `#[async_trait]` erases the implementation from the signature that would
/// have made the gap visible. The trait is now ten methods, each with at
/// least one caller.
///
/// The name is asserted absent for the same reason the other names are
/// asserted present: a cut that regrows is a cut that did not happen.
#[test]
fn the_cut_task_store_method_stays_cut() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/http/tasks/state.rs");
    let text = fs::read_to_string(&path).expect("state.rs is readable");
    assert!(
        !text.contains("update_progress_fenced"),
        "`TaskStore::update_progress_fenced` is back. It was cut because nothing \
         called it: reporting progress has to be written by whoever owns the \
         task, and no caller here is that owner. If a worker now needs to report \
         progress, wire the call that needs it — restoring the method on its own \
         is the state it was cut from."
    );
}
