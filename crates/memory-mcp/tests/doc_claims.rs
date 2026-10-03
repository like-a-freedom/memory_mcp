//! Documentation claims that nothing else checks.
//!
//! A document that cites a file which does not exist is not merely untidy: it
//! tells the next reader that the reasoning behind a decision lives somewhere
//! they can go and read, and there is nothing there. ADR-0064 removed the check
//! that used to catch this, because the audit it ran exited early on an empty
//! `plans/` directory. The directory is no longer empty, and ADR-0065 reinstates
//! the check as a cargo test — the only shape ADR-0064 permits CI to run.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

fn workspace_root() -> PathBuf {
    // CARGO_MANIFEST_DIR is crates/memory-mcp; the docs and the ADRs are the
    // workspace's, not this crate's.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("workspace root is canonicalizable")
}

/// Every markdown file whose links are a claim about this repository.
fn documented_files() -> Vec<PathBuf> {
    let root = workspace_root();
    let mut files = Vec::new();

    for name in ["AGENTS.md", "README.md", "CONTEXT.md"] {
        let path = root.join(name);
        if path.is_file() {
            files.push(path);
        }
    }

    let docs = root.join("docs");
    let mut stack = vec![docs];
    while let Some(current) = stack.pop() {
        let Ok(entries) = fs::read_dir(&current) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|ext| ext == "md") {
                files.push(path);
            }
        }
    }

    files.sort();
    assert!(!files.is_empty(), "found no markdown to check");
    files
}

#[test]
fn every_relative_markdown_link_resolves() {
    let mut dangling: BTreeSet<String> = BTreeSet::new();

    for file in documented_files() {
        let text = read(&file);
        for target in markdown_links(&text) {
            let Some(relative) = relative_target(&target) else {
                continue;
            };
            if !resolve_citation(&file, relative).exists() {
                dangling.insert(format!(
                    "{}: [{}]({})",
                    display(&file),
                    link_text(&text, &target),
                    target
                ));
            }
        }
    }

    assert!(
        dangling.is_empty(),
        "{} markdown link(s) point at a file that does not exist. Point them at \
         the record that actually survives, or drop the citation:\n\n{}",
        dangling.len(),
        dangling.iter().cloned().collect::<Vec<_>>().join("\n")
    );
}

/// Every ADR's link and backticked `docs/…` reference resolves.
///
/// ADRs are the layer where a reader goes to find out *why*, so a broken
/// reference in one is worse than in a working note: it points at the
/// evidence for a decision and the evidence is not there.
#[test]
fn every_adr_cites_an_existing_file() {
    let adr_dir = workspace_root().join("docs/adr");
    let mut dangling: BTreeSet<String> = BTreeSet::new();

    let mut adrs: Vec<PathBuf> = fs::read_dir(&adr_dir)
        .expect("docs/adr is readable")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "md"))
        .collect();
    adrs.sort();
    assert!(!adrs.is_empty(), "found no ADRs to check");

    for adr in &adrs {
        let text = read(adr);
        let mut claims: Vec<String> = markdown_links(&text)
            .into_iter()
            .filter_map(|t| relative_target(&t).map(str::to_string))
            .collect();
        claims.extend(backticked_doc_paths(&text));
        for claim in claims {
            if !resolve_citation(adr, &claim).exists() {
                dangling.insert(format!("{}: {claim}", display(adr)));
            }
        }
    }

    assert!(
        dangling.is_empty(),
        "{} reference(s) in the ADRs do not resolve:\n\n{}",
        dangling.len(),
        dangling.iter().cloned().collect::<Vec<_>>().join("\n")
    );
}

/// A spec's `**Status:**` line must be one of three exact values, and a spec
/// claiming `Implemented` must name the ADRs that carry the decision.
#[test]
fn every_spec_status_line_matches_the_code() {
    let specs_dir = workspace_root().join("docs/superpowers/specs");
    let mut specs: Vec<PathBuf> = fs::read_dir(&specs_dir)
        .expect("docs/superpowers/specs is readable")
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.extension().is_some_and(|ext| ext == "md"))
        .collect();
    specs.sort();
    assert!(!specs.is_empty(), "found no specs to check");

    let expected = expected_spec_statuses();
    let mut problems: Vec<String> = Vec::new();

    for spec in &specs {
        let name = spec
            .file_name()
            .and_then(|n| n.to_str())
            .expect("spec filename is UTF-8")
            .to_string();
        let Some(want) = expected.get(name.as_str()) else {
            problems.push(format!(
                "{name}: no expected status in the table in doc_claims.rs. Add one \
                 with a reason — an unlisted spec is a spec nobody is reviewing."
            ));
            continue;
        };

        let text = read(spec);
        let Some(line) = text.lines().find(|l| status_value(l).is_some()) else {
            problems.push(format!("{name}: has no `**Status:**` line"));
            continue;
        };
        let got = status_value(line)
            .expect("just found the status line")
            .to_string();
        if &got != want {
            problems.push(format!(
                "{name}: status is `{got}`, the table expects `{want}`. Either the \
                 code moved on and the spec did not, or the table is wrong."
            ));
            continue;
        }

        if *want == "Implemented" {
            let cited = cited_adrs(&text);
            if cited.is_empty() {
                problems.push(format!(
                    "{name}: status is `Implemented` but names no ADR. An \
                     implemented decision that no ADR records cannot be found, \
                     audited, or reversed."
                ));
            }
            let existing = existing_adrs();
            for number in cited.difference(&existing) {
                problems.push(format!(
                    "{name}: cites ADR-{number:04}, which does not exist"
                ));
            }
        }
    }

    assert!(
        problems.is_empty(),
        "{} spec status problem(s):\n\n{}",
        problems.len(),
        problems.join("\n")
    );
}

/// The status each spec is expected to carry, and why.
///
/// This is a table rather than a derivation because the assertion cannot be
/// derived: the whole question is whether the document matches the code, and
/// the code is the thing under audit. Adding a row is a claim that somebody
/// has checked.
fn expected_spec_statuses() -> BTreeMap<&'static str, &'static str> {
    BTreeMap::from([
        (
            "2026-07-28-truthful-evaluation-system-design.md",
            "Implemented",
        ),
        (
            "2026-07-30-token-efficient-responses-design.md",
            "Implemented",
        ),
        (
            "2026-08-27-background-gliner-refresh-design.md",
            "Implemented",
        ),
        ("2026-08-27-streamable-http-saas.md", "Implemented"),
        (
            "2026-09-02-architecture-audit-remediation-design.md",
            "Implemented",
        ),
        ("2026-09-18-local-admin-auth.md", "Implemented"),
        ("2026-09-23-ddd-modular-monolith.md", "Implemented"),
        ("2026-09-23-path-prefix-deployment.md", "Implemented"),
        (
            "2026-09-30-architecture-audit-remediation.md",
            "Accepted direction",
        ),
        (
            "2026-10-03-architecture-audit-follow-up.md",
            "Accepted direction",
        ),
    ])
}

/// The ADR numbers a document cites, from every `ADR-0012`-shaped token.
///
/// The assertion checks the *number*, because that is what a citation asserts:
/// the file `0012-…md` may be renamed, and a spec that says `ADR-0012` is
/// pointing at a record, not at a filename.
fn cited_adrs(text: &str) -> BTreeSet<u32> {
    let mut found = BTreeSet::new();
    let mut rest = text;
    while let Some(at) = rest.find("ADR-") {
        rest = &rest[at + 4..];
        let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
        if digits.len() == 4
            && let Ok(number) = digits.parse::<u32>()
        {
            found.insert(number);
        }
    }
    found
}

/// The ADR files that exist, by number.
fn existing_adrs() -> BTreeSet<u32> {
    let adr_dir = workspace_root().join("docs/adr");
    fs::read_dir(&adr_dir)
        .expect("docs/adr is readable")
        .flatten()
        .filter_map(|entry| {
            let name = entry.file_name();
            let name = name.to_str()?;
            let digits: String = name.chars().take_while(char::is_ascii_digit).collect();
            digits.parse::<u32>().ok()
        })
        .collect()
}

/// The value of a `Status:` line, whether or not the line bolds the key.
///
/// The earliest spec writes `Status:` in prose style and the rest write
/// `**Status:**`. Both are the same claim, so the guard reads both rather than
/// forcing a cosmetic edit on a document nobody is otherwise touching.
fn status_value(line: &str) -> Option<&str> {
    let trimmed = line.trim_start();
    trimmed
        .strip_prefix("**Status:**")
        .or_else(|| trimmed.strip_prefix("Status:"))
        .map(str::trim)
}

fn read(path: &Path) -> String {
    fs::read_to_string(path).unwrap_or_else(|e| panic!("{} is not readable: {e}", path.display()))
}

fn display(path: &Path) -> String {
    path.strip_prefix(workspace_root())
        .unwrap_or(path)
        .display()
        .to_string()
}

/// The `](…)` targets of every markdown link in `text`.
///
/// Only `](` is matched, and only outside code — fenced blocks and inline
/// spans. That is what keeps the guard from reading its own documentation as
/// a claim: this plan describes the extraction step using `[text](target)` as
/// the example of a link, and a guard that reported that would have to be
/// weakened to survive its own repo. The same rule is what lets ADR-0064:38
/// describe `docs/superpowers/plans` in backticks as part of the true record
/// of why an audit once exited, with no "fix" required.
fn markdown_links(text: &str) -> Vec<String> {
    let mut targets = Vec::new();
    let mut in_fence = false;
    for line in text.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence {
            continue;
        }
        let mut i = 0;
        while let Some(found) = line[i..].find("](") {
            let open = i + found + 2;
            let Some(close_offset) = line[open..].find(')') else {
                break;
            };
            let close = open + close_offset;
            let target = line[open..close].trim();
            if !target.is_empty() && is_simple_link_target(target) && !inside_code_span(line, open)
            {
                targets.push(target.to_string());
            }
            i = close;
        }
    }
    targets
}

/// Whether the `](` at `at` sits inside a backticked span on this line.
///
/// Backticks pair off left to right, so an odd count before the `](` means a
/// span is open and the text is an example rather than a rendered link. This
/// is what stops the guard reading the plan that specifies it: the plan writes
/// `[text](target)` in backticks to name the shape it extracts, and a guard
/// that reported that would have to be weakened to survive its own repo.
fn inside_code_span(line: &str, at: usize) -> bool {
    line[..at].matches('`').count() % 2 == 1
}

/// Reject the forms a markdown parser resolves that a filesystem check
/// cannot: a title after the target, and an angle-bracketed target.
fn is_simple_link_target(target: &str) -> bool {
    if target.contains('\n') || target.starts_with('<') {
        return false;
    }
    let mut parts = target.split_whitespace();
    let first = parts.next().unwrap_or_default();
    if first != target && !first.ends_with('>') {
        return false;
    }
    true
}

fn link_text(text: &str, target: &str) -> String {
    let needle = format!("]({target})");
    text.lines()
        .find(|line| line.contains(&needle))
        .and_then(|line| {
            let at = line.find(&needle)?;
            let open = line[..at].rfind('[')?;
            Some(line[open + 1..at].to_string())
        })
        .unwrap_or_default()
}

/// The path part of a link target, or `None` for the forms that name no file
/// on disk: absolute URLs, in-page anchors, and `mailto:`.
fn relative_target(target: &str) -> Option<&str> {
    let target = target.split_whitespace().next().unwrap_or(target);
    if target.starts_with("http://")
        || target.starts_with("https://")
        || target.starts_with('#')
        || target.starts_with("mailto:")
        || target.is_empty()
    {
        return None;
    }
    Some(target)
}

/// Resolve a cited path the way a reader would.
///
/// A bare `docs/…` is written from the repository root — that is the form
/// ADRs and specs use in backticks. A `../`-prefixed or `./`-prefixed path is
/// relative to the file that cites it, which is the form a markdown link
/// takes. Resolving both against the citing file's directory would report
/// every `docs/…` in an ADR as broken, which is how a guard loses its
/// reader: 25 findings, all false, is a guard that gets deleted.
fn resolve_citation(citing_file: &Path, target: &str) -> PathBuf {
    let base = target.split('#').next().unwrap_or(target);
    if base.starts_with("docs/") || base.starts_with("crates/") || base.starts_with("scripts/") {
        workspace_root().join(base)
    } else {
        citing_file.parent().unwrap_or(Path::new(".")).join(base)
    }
}

/// `docs/…` paths written inside backticks, which is how ADRs and specs cite a
/// file without turning it into a link.
///
/// A token has to look like a path to count. `docs/…` is an ellipsis standing
/// for "the documentation tree", and this ADR uses the phrase in prose; taking
/// it as a citation would make the guard report its own description of itself
/// as a broken reference.
fn backticked_doc_paths(text: &str) -> Vec<String> {
    let mut found = Vec::new();
    let mut rest = text;
    while let Some(open) = rest.find('`') {
        let after = &rest[open + 1..];
        let Some(close) = after.find('`') else {
            break;
        };
        let inner = &after[..close];
        let looks_like_a_path = !inner.is_empty()
            && !inner.contains(char::is_whitespace)
            && !inner.contains('…')
            && !inner.contains("..");
        if looks_like_a_path && (inner.starts_with("docs/") || inner.contains("/docs/")) {
            found.push(inner.to_string());
        }
        rest = &after[close + 1..];
    }
    found
}
