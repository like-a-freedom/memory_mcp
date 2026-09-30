//! One implementation per concern, so a fix lands in one place.
//!
//! The 2026-09-30 audit found nine duplicated pairs in this crate. Five were
//! mechanical; four were not, and the four that were not are the interesting
//! ones. This test pins the five, and pins two of the four as *absences* so a
//! later reader does not "fix" a deliberate non-merge.
//!
//! The absences matter as much as the presences. `ranking.rs`'s raw
//! `split_whitespace()` and `search_query_terms` are different operations:
//! routing the ranker through the shared normaliser would change which
//! procedure candidates match, silently, with every unit test still green.
//! Three different ranking paths exist on purpose. A guard that cannot tell
//! a duplicate from a coincidence will eventually remove the wrong one.

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

/// Concerns whose implementation must exist in exactly one file.
///
/// `None` as the canonical path means the name must not exist anywhere: the
/// duplicate was removed by deleting the name itself, not by pointing two
/// call sites at one survivor.
const CANONICAL: &[(&str, Option<&str>)] = &[
    ("normalize_surreal_json", Some("src/storage/helpers.rs")),
    (
        "apply_nms",
        Some("src/knowledge/entity_extraction/lfm2_gliner/decode.rs"),
    ),
    ("ScoredSpan", None),
    ("lexical_record_term_set", None),
];

/// Names that are *not* duplicates, kept deliberately, with the reason.
///
/// These are the ones the audit's first pass got wrong. Each looked like a
/// copy of something in `shared/search_lexical.rs` and is not: a grep result
/// generalises, a read does not. Recording them is what stops the next reader
/// from "consolidating" one and silently changing retrieval.
const NOT_DUPLICATES: &[(&str, &str, &str)] = &[
    (
        "matched_query_terms_for_record",
        "src/memory/retrieval/lexical.rs",
        "It collects `best_matching_content_terms`, which picks the best-scoring \
         span of the content and narrows the match to that span. The shared \
         `matched_query_terms_for_fact` unions every term in the content. Those \
         answer different questions — 'which span of this record matches' and \
         'which terms does this record contain' — and the span score feeds the \
         ranking below it. Routing one through the other would change which \
         candidates rank higher, with every unit test still green.",
    ),
    (
        "lexical_query_overlap_for_fact",
        "src/memory/retrieval/lexical.rs",
        "A `Fact`-typed entry point beside the `Value`-typed `lexical_query_overlap`. \
         One operation, two input types: the `Value` form parses the record first, \
         and the `Fact` form does not have to.",
    ),
];

/// A rename that must not be undone, with the file it lives in and why the two
/// same-named things are not the same thing.
///
/// `PolicyFingerprint::compute_v2` hashes a scope, a project and a tag set;
/// this is a sorted, comma-joined tag set, written verbatim into
/// `ExposureTrace.policy_fingerprint` and into the `RecallKey` string a reader
/// sees in a trace. It was renamed `policy_tag_set_key`. Hashing it would make
/// the stored trace unreadable, and the recall cache keys on the tag set, not
/// on a digest.
const DELIBERATE_RENAMES: &[(&str, &str, &str)] = &[(
    "policy_tag_set_key",
    "src/memory/agent_memory/recall.rs",
    "It is a sorted, comma-joined tag set used verbatim in \
     `ExposureTrace.policy_fingerprint` and in the `RecallKey` string. The pure \
     `PolicyFingerprint::compute_v2` hashes a scope, a project and a tag set; \
     forcing this one through that hash would make the stored trace unreadable, \
     and the recall cache keys on the tag set, not on a digest.",
)];

fn source_files() -> Vec<PathBuf> {
    let src = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut found = Vec::new();
    let mut stack = vec![src];
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
    assert!(!found.is_empty(), "no sources found");
    found
}

/// Every `fn <name>` / `struct <name>` / `type <name>` definition of `name`,
/// as workspace-relative paths.
fn definitions(files: &[PathBuf], name: &str) -> Vec<String> {
    let mut out = Vec::new();
    for file in files {
        let Ok(text) = fs::read_to_string(file) else {
            continue;
        };
        for keyword in ["fn ", "struct ", "enum ", "type "] {
            for line in text.lines() {
                let trimmed = line.trim_start();
                let Some(rest) = trimmed
                    .strip_prefix("pub ")
                    .or_else(|| trimmed.strip_prefix("pub(crate) "))
                    .unwrap_or(trimmed)
                    .strip_prefix(keyword)
                else {
                    continue;
                };
                if rest
                    .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                    .next()
                    == Some(name)
                    && trimmed.starts_with("fn ")
                    || rest
                        .split(|c: char| !(c.is_alphanumeric() || c == '_'))
                        .next()
                        == Some(name)
                {
                    out.push(relative(file));
                    break;
                }
            }
        }
    }
    out.sort();
    out.dedup();
    out
}

fn relative(path: &Path) -> String {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    path.strip_prefix(manifest)
        .unwrap_or(path)
        .display()
        .to_string()
}

#[test]
fn each_concern_has_exactly_one_implementation() {
    let files = source_files();
    let mut problems = Vec::new();

    for (name, canonical) in CANONICAL {
        let found = definitions(&files, name);
        match canonical {
            None => {
                if !found.is_empty() {
                    problems.push(format!(
                        "{name} must not exist anywhere — the duplicate was removed by \
                         deleting the name. It is defined in: {found:?}"
                    ));
                }
            }
            Some(expected) => {
                if found.len() != 1 {
                    problems.push(format!(
                        "{name} must be defined exactly once, in {expected}. Found {} \
                         definition(s): {found:?}. A second copy means a fix to one \
                         will not reach the other.",
                        found.len()
                    ));
                } else if &found[0] != expected {
                    problems.push(format!(
                        "{name} is defined in {}, but {expected} is the file that owns it. \
                         Either the move was half-done or the table is stale.",
                        found[0]
                    ));
                }
            }
        }
    }

    assert!(
        problems.is_empty(),
        "{} duplicate-implementation problem(s):\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}

#[test]
fn a_deliberate_rename_is_still_where_it_was_put() {
    let files = source_files();
    let mut problems = Vec::new();

    for (name, file, reason) in DELIBERATE_RENAMES {
        let found = definitions(&files, name);
        if !found.iter().any(|f| f == file) {
            problems.push(format!(
                "{name} is missing from {file}. It was renamed, not deleted — {reason}"
            ));
        }
    }

    assert!(
        problems.is_empty(),
        "{} deliberate rename(s) were undone:\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}

#[test]
fn a_name_that_looks_duplicated_but_is_not_stays_where_it_is() {
    let files = source_files();
    let mut problems = Vec::new();

    for (name, file, reason) in NOT_DUPLICATES {
        let found = definitions(&files, name);
        if !found.iter().any(|f| f == file) {
            problems.push(format!(
                "{name} is missing from {file}. It looked like a duplicate of a \
                 shared helper and is not — {reason}"
            ));
        }
    }

    assert!(
        problems.is_empty(),
        "{} function(s) that are not duplicates were removed:\n\n{}",
        problems.len(),
        problems.join("\n\n")
    );
}

/// The three ranking paths are three different objectives, and a fourth
/// consolidation is the mistake a reader is most likely to make.
///
///  * `memory/retrieval/ranking.rs` — RRF fusion, ranking facts for the
///    assembled context.
///  * `memory/retrieval/temporal.rs` — lexical plus recency, ordering
///    candidates inside a temporal window.
///  * `procedures_service/ranking.rs` — posterior mean, ranking procedure
///    candidates.
///
/// The assertion is that all three files still exist and still name their
/// objective. It cannot prove the objectives are distinct; it can stop the
/// files disappearing, which is the failure that actually happens.
#[test]
fn the_three_ranking_paths_still_exist() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let expected: BTreeMap<&str, &str> = BTreeMap::from([
        ("src/memory/retrieval/ranking.rs", "fusion"),
        ("src/memory/retrieval/temporal.rs", "temporal"),
        ("src/memory/procedures_service/ranking.rs", "procedure"),
    ]);

    let mut problems = Vec::new();
    for (path, objective) in &expected {
        let full = manifest.join(path);
        match fs::read_to_string(&full) {
            Ok(text) if text.contains(objective) => {}
            Ok(_) => problems.push(format!(
                "{path} no longer names its objective ({objective}). Three ranking \
                 paths are deliberate; if one was merged, this ADR-sized judgement \
                 needs recording, not a silent deletion."
            )),
            Err(_) => problems.push(format!(
                "{path} is gone. The {objective} ranking path is missing."
            )),
        }
    }

    assert!(
        problems.is_empty(),
        "the deliberate ranking split changed:\n\n{}",
        problems.join("\n")
    );
}
