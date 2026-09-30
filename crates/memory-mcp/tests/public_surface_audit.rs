//! Ratchets on the public surface, so dead re-exports cannot regrow.
//!
//! `service.rs` once carried a block of `pub use` re-exports with no caller
//! outside the module. Nothing reported them: a re-export is a name, not code,
//! so rustc emits no warning and no test fails. The block was removed in the
//! 2026-09-30 audit remediation, and this test is what keeps it removed.
//!
//! It is a ratchet rather than a prohibition. A handful of names in that block
//! *do* have consumers — a few tests, a few transport adapters — and removing
//! them would be churn, not cleanliness. They are listed below with the reason
//! each one earns its place. A new name is added to the allowlist deliberately,
//! with a reason, or not at all.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

/// The re-exported names that still have a consumer, and where it is.
///
/// Every entry is a claim that can be checked: delete the consumer and the
/// name belongs on the cut list, not in this table.
const REEXPORTS_WITH_CONSUMERS: &[(&str, &str)] = &[
    ("build_extract_log_result", "src/tools/extract.rs"),
    ("edge_neighbor", "src/mcp/handlers.rs"),
    ("fact_from_record", "tests/apps_ingestion_review.rs"),
    ("graph_neighbor_expansion", "src/service/apps/dispatch.rs"),
    ("graph_payload", "src/mcp/handlers/apps.rs"),
    ("LifecycleOperation", "src/service/cli/lifecycle.rs"),
    ("normalize_text", "tests/common/mod.rs"),
];

/// The names the audit found re-exported with no consumer outside `service.rs`.
///
/// This is the ratchet. The test asserts that none of them is re-exported
/// again, and — the part that makes it a ratchet rather than a wall — that
/// every name in the allowlist above is still re-exported, so the table cannot
/// quietly accumulate entries for names that have since been cut.
const CUT_REEXPORTS: &[&str] = &[
    "Fact",
    "bucket_to_five_minutes",
    "bucket_to_hour",
    "decayed_confidence",
    "deterministic_community_id",
    "deterministic_episode_id",
    "deterministic_fact_id",
    "episode_from_record",
    "hash_prefix",
    "ids",
    "parse_iso",
    "preprocess_search_query",
    "validate_entity_candidate",
    "validate_fact_input",
    "validate_ingest_request",
];

fn service_module() -> (PathBuf, String) {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/service.rs");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} is not readable: {e}", path.display()));
    (path, text)
}

/// The names `src/service.rs` re-exports, from every `pub use` /
/// `pub(crate) use` line.
///
/// A multi-line `use` is joined before parsing, because the block this test
/// is about wraps most of its names onto continuation lines — and a parser
/// that read one line at a time would see a clause ending in `,` and quietly
/// miss every name in it. That failure mode is silent, so it is not one worth
/// having.
fn reexported_names(text: &str) -> BTreeSet<String> {
    let mut joined = String::new();
    for line in strip_comments(text).lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("//") {
            continue;
        }
        joined.push_str(trimmed);
        if !trimmed.ends_with(';') {
            joined.push(' ');
        }
    }

    let mut names = BTreeSet::new();
    let mut rest = joined.as_str();
    while let Some(at) = rest.find("use ") {
        rest = &rest[at + 4..];
        let end = rest.find(';').unwrap_or(rest.len());
        let clause = rest[..end].trim();
        rest = &rest[end.min(rest.len())..];
        // A glob names nothing in particular, and is not part of the block
        // this test is about.
        if clause.is_empty() || clause.ends_with('*') {
            continue;
        }
        // The last path segment is the name being brought into scope; a
        // trailing `as Alias` renames it.
        let leaf = clause
            .rsplit("::")
            .next()
            .unwrap_or_default()
            .split(" as ")
            .next()
            .unwrap_or_default()
            .trim();
        for part in leaf.split(',') {
            let name = part.trim();
            if !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_') {
                names.insert(name.to_string());
            }
        }
    }
    names
}

/// Blank out comments so a `pub use` mentioned in prose is not read as a
/// declaration.
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

#[test]
fn no_cut_name_is_re_exported_again() {
    let (_, text) = service_module();
    let present = reexported_names(&text);

    let regrown: Vec<&&str> = CUT_REEXPORTS
        .iter()
        .filter(|name| present.contains(**name))
        .collect();

    assert!(
        regrown.is_empty(),
        "these names were re-exported by `service` with no consumer and must \
         stay cut. Import from the owning module instead:\n{regrown:#?}"
    );
}

/// Whether `text` reaches `name` through the `service` module.
///
/// Both forms count: a `use` that brings the name into scope, and a qualified
/// call that names it in place. A `use` clause is joined before it is read,
/// because the interesting imports wrap across lines —
/// `apps_ingestion_review.rs` names three items over two lines, and a
/// line-at-a-time check would score the last one as unused.
fn imports_through_service(text: &str, name: &str) -> bool {
    let mut joined = String::new();
    for line in text.lines() {
        let trimmed = line.trim_start();
        joined.push_str(trimmed);
        if !trimmed.ends_with(';') && !trimmed.ends_with(',') {
            joined.push(' ');
        }
    }

    let mut rest = joined.as_str();
    while let Some(at) = rest.find("::service::") {
        rest = &rest[at + "::service::".len()..];
        if rest.starts_with(name) {
            return true;
        }
        let end = rest.find(';').unwrap_or(rest.len());
        let clause = rest[..end].trim();
        rest = &rest[end.min(rest.len())..];
        let items = clause
            .trim_start_matches('{')
            .trim_end_matches('}')
            .trim_end_matches(';');
        for item in items.split(',') {
            if item.split_whitespace().next().unwrap_or("").trim() == name {
                return true;
            }
        }
    }
    false
}

#[test]
fn the_allowlist_names_a_consumer_that_exists() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let mut problems = Vec::new();

    for (name, consumer) in REEXPORTS_WITH_CONSUMERS {
        let path = manifest.join(consumer);
        if !path.is_file() {
            problems.push(format!("{name}: {consumer} does not exist"));
            continue;
        }
        let text = fs::read_to_string(&path).expect("consumer is readable");
        if !imports_through_service(&text, name) {
            problems.push(format!(
                "{name}: {consumer} no longer imports it through `service`. Either \
                 the name belongs on the cut list, or the entry names the wrong file."
            ));
        }
    }

    assert!(
        problems.is_empty(),
        "{} allowlist entr(ies) no longer hold:\n\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// `decayed_confidence` is a name the audit cut, and keeping it cut is a
/// judgement worth pinning: a live function with the same job still exists at
/// `memory/retrieval.rs`, and a future reader who finds the gap may "restore"
/// the wrapper.
#[test]
fn the_live_decay_function_is_not_the_cut_one() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/service/query.rs");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} is not readable: {e}", path.display()));
    assert!(
        !text.contains("fn decayed_confidence"),
        "`service::query::decayed_confidence` is back. The live function is \
         `memory::retrieval::fact_decayed_confidence`, which is the injection \
         point that makes decay substitutable in tests; the wrapper was a \
         second name for it and had no caller but its own tests."
    );
}
