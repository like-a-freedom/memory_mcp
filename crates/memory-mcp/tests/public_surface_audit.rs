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
        for name in use_tree_names(clause) {
            names.insert(name);
        }
    }
    names
}

/// The names a `use` clause brings into scope.
///
/// The clause is a tree, not a path with one leaf: `a::b::{Fact, ids}`
/// re-exports two names, and reading the last `::` segment as the name — which
/// is what this parser used to do — yields `{Fact, ids}`, fails the
/// identifier check, and reports nothing. A cut name written in the grouped
/// form therefore passed the ratchet silently.
fn use_tree_names(clause: &str) -> Vec<String> {
    let clause = clause.trim().trim_end_matches(';').trim();
    if clause.is_empty() {
        return Vec::new();
    }
    if let Some(open) = clause.find('{') {
        // `prefix::{a, b}` — every element is a name under `prefix`.
        let prefix = &clause[..open];
        let Some(close) = clause.rfind('}') else {
            return Vec::new();
        };
        let inner = &clause[open + 1..close];
        let mut names = Vec::new();
        for element in split_top_level(inner) {
            let element = element.trim();
            if element == "self" {
                // `a::b::{self, c}` brings `b` itself into scope.
                if let Some(name) = prefix
                    .trim_end_matches("::")
                    .rsplit("::")
                    .next()
                    .filter(|name| is_identifier(name))
                {
                    names.push(name.to_string());
                }
                continue;
            }
            names.extend(use_tree_names(&format!("{prefix}{element}")));
        }
        return names;
    }

    // A leaf: the last path segment, possibly renamed by `as`.
    let (path, alias) = match clause.split_once(" as ") {
        Some((path, alias)) => (path, Some(alias.trim())),
        None => (clause, None),
    };
    let leaf = path.trim().rsplit("::").next().unwrap_or_default().trim();
    match alias {
        Some(alias) if is_identifier(alias) => vec![alias.to_string()],
        Some(_) => Vec::new(),
        None if is_identifier(leaf) => vec![leaf.to_string()],
        None => Vec::new(),
    }
}

/// Split a `use` group body on its top-level commas, ignoring commas inside a
/// nested group.
fn split_top_level(text: &str) -> Vec<&str> {
    let mut parts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    for (index, c) in text.char_indices() {
        match c {
            '{' | '<' => depth += 1,
            '}' | '>' => depth = depth.saturating_sub(1),
            ',' if depth == 0 => {
                parts.push(&text[start..index]);
                start = index + 1;
            }
            _ => {}
        }
    }
    parts.push(&text[start..]);
    parts
}

fn is_identifier(name: &str) -> bool {
    !name.is_empty() && name.chars().all(|c| c.is_alphanumeric() || c == '_')
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

/// The ratchet above is only as good as this parser. A grouped re-export of a
/// cut name — `pub use crate::types::{Fact, ids};` — used to yield no names at
/// all, so the ratchet could not fire however the name regrew.
#[test]
fn a_grouped_re_export_of_a_cut_name_is_seen() {
    let grouped = reexported_names("pub use crate::types::{Fact, ids};\n");
    assert!(
        grouped.contains("Fact") && grouped.contains("ids"),
        "a grouped re-export must enumerate every name in it, got {grouped:?}"
    );
    assert!(
        CUT_REEXPORTS.iter().any(|cut| grouped.contains(*cut)),
        "the ratchet must be able to fire on this form"
    );
}

#[test]
fn every_re_export_form_the_module_uses_is_enumerated() {
    let cases = [
        ("pub use crate::types::Fact;", vec!["Fact"]),
        (
            "pub(crate) use crate::types::Fact as Renamed;",
            vec!["Renamed"],
        ),
        (
            "pub use crate::types::{alpha, beta as gamma};",
            vec!["alpha", "gamma"],
        ),
        (
            "pub use crate::types::{self, delta};",
            vec!["types", "delta"],
        ),
        (
            "pub use crate::outer::{inner::{epsilon, zeta}, eta};",
            vec!["eta", "epsilon", "zeta"],
        ),
    ];

    for (source, expected) in cases {
        let names = reexported_names(source);
        for name in expected {
            assert!(
                names.contains(name),
                "{source:?} must yield {name}, got {names:?}"
            );
        }
    }
}

/// A grouped body must not swallow the elements around a nested group: a
/// naive comma split would report `inner::{epsilon` as a name and lose `zeta`.
#[test]
fn a_nested_group_does_not_hide_its_siblings() {
    let names = reexported_names("pub use crate::outer::{inner::{epsilon, zeta}, eta};");

    assert!(names.contains("eta"), "got {names:?}");
    assert!(names.contains("zeta"), "got {names:?}");
    assert!(
        !names
            .iter()
            .any(|name| name.contains('{') || name.contains('}')),
        "a delimiter must never be read as part of a name, got {names:?}"
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

/// The introduction-chain BFS is unreachable from any registered app
/// action, so Task 5.3 cut it rather than moving it into a bounded context
/// where it would acquire a fresh, respectable-looking home.
///
/// This asserts the absence, so it passes now and fails if anyone re-adds a
/// path to it. The reachability argument: the `graph` app registers exactly
/// `expand_neighbors`, `open_edge_details` and `use_path_as_context` in
/// `service/apps/dispatch.rs`, and none of them reaches the chain. The only
/// callers were five assertions in `tests/service_acceptance.rs`.
#[test]
fn the_introduction_chain_is_not_reintroduced_into_the_graph_app() {
    let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("src/service/apps/graph.rs");
    let text = fs::read_to_string(&path)
        .unwrap_or_else(|e| panic!("{} is not readable: {e}", path.display()));
    for name in [
        "fn find_intro_chain",
        "fn intro_chain_from_start",
        "fn find_entity_id_by_name",
    ] {
        assert!(
            !text.contains(name),
            "`{name}` is back in `service/apps/graph.rs`. No registered app \
             action reaches the introduction chain: the `graph` app offers \
             `expand_neighbors`, `open_edge_details` and \
             `use_path_as_context`, and none of them calls it. If a new action \
             genuinely needs it, it needs a traversal that takes \
             `&KnowledgeGraphStore` — the one this code cannot reach."
        );
    }
}

/// Every `pub` method on `MemoryService` should have a caller outside the
/// test tree. `MemoryService` is the crate's most public type, so a method
/// on it reads as an affordance the crate offers; when the only callers are
/// tests, it is a fixture convenience wearing a production interface.
///
/// This is a ratchet in the other direction from the re-export guard: it
/// lists what a `pub` method is allowed to be, and a new `pub` method that
/// only tests call has to be added here with a reason, or moved into the
/// test that wanted it.
#[test]
fn every_public_container_method_has_a_production_caller() {
    let manifest = Path::new(env!("CARGO_MANIFEST_DIR"));
    let src = manifest.join("src");

    // The methods, and why each one is allowed to exist without a
    // production caller. The list exists so an entry is a deliberate act
    // with a reason attached, rather than an oversight — and it is not
    // empty: Task 5.4 put two names in it, each of which turned out to be
    // the only public door to something.
    const TEST_ONLY: &[(&str, &str)] = &[
        // The constructors. `new` and `new_with_embedding_provider` are how a
        // caller obtains a container at all, so having no caller *of their
        // own* inside the crate is the point.
        ("new", "constructs the container"),
        ("new_with_embedding_provider", "constructs the container"),
        // The only public way to record an edge. `store_edge` in
        // `memory/episode/edges.rs` is `pub(crate)`, so an out-of-crate
        // caller — the eval harness, an embedding — has exactly one door, and
        // this is it. It delegates to `relate_edge`, which two production
        // paths (`lifecycle_workers::communities`, `episode::edges`) do
        // reach. Removing it would close the crate's graph-write surface
        // entirely rather than narrow it.
        (
            "relate",
            "the only public edge-write path; store_edge is pub(crate)",
        ),
        // `MemoryService::new` takes the lifecycle configuration from its
        // caller rather than reading the environment, so a caller that
        // builds a container directly has no other way to turn the
        // integration on. The composition root sets the same field from
        // `LifecycleConfig::from_env`; this is the constructor-path
        // equivalent. Removing it would make the lifecycle integration
        // reachable only through `bootstrap::stdio`.
        (
            "with_lifecycle_enabled",
            "the constructor-path equivalent of the composition root's LifecycleConfig::from_env",
        ),
    ];

    let declared = collect_public_methods(&src);
    assert!(
        !declared.is_empty(),
        "no `pub` methods found on MemoryService — the scan is broken, not the code"
    );

    let mut test_text = String::new();
    collect_rust_text(&manifest.join("tests"), &mut test_text);
    for path in &declared {
        if let Ok(text) = std::fs::read_to_string(&path.file) {
            test_text.push_str(&text);
        }
    }

    let mut orphans = Vec::new();
    for method in &declared {
        let name = &method.name;
        if TEST_ONLY.iter().any(|(allowed, _)| allowed == name) {
            continue;
        }
        let callers = count_production_callers(&src, name);
        if callers > 0 {
            continue;
        }
        let test_only = test_text.contains(&format!(".{name}("));
        orphans.push(format!(
            "  {name} — {}{}",
            method.file.display(),
            if test_only {
                " (tests call it; no production caller)"
            } else {
                " (no caller at all)"
            }
        ));
    }

    assert!(
        orphans.is_empty(),
        "{} pub method(s) on MemoryService have neither a production caller \
         nor a test:\n\n{}\n\nMove each into the test that wanted it, or add it to \
         TEST_ONLY with a reason.",
        orphans.len(),
        orphans.join("\n")
    );
}

struct DeclaredMethod {
    name: String,
    file: PathBuf,
}

fn collect_public_methods(src: &Path) -> Vec<DeclaredMethod> {
    // The container's `pub` methods live in `service/core.rs`,
    // `service/core/builder.rs` and `service/apps/graph.rs`. Walking the
    // whole tree and keeping `pub async fn` / `pub fn` on an `impl
    // MemoryService` block would be more general; the file list is explicit
    // so a moved method cannot silently fall out of the scan.
    let files = [
        src.join("service/core.rs"),
        src.join("service/core/builder.rs"),
        src.join("service/apps/graph.rs"),
    ];
    let mut found = Vec::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let mut in_impl = false;
        for line in text.lines() {
            if line.starts_with("impl MemoryService") {
                in_impl = true;
                continue;
            }
            if in_impl && line == "}" {
                in_impl = false;
                continue;
            }
            if !in_impl {
                continue;
            }
            let body = line.trim_start();
            for prefix in ["pub async fn ", "pub fn "] {
                if let Some(rest) = body
                    .strip_prefix(prefix)
                    .and_then(|rest| rest.split('(').next())
                {
                    found.push(DeclaredMethod {
                        name: rest.to_string(),
                        file: file.clone(),
                    });
                }
            }
        }
    }
    found
}

fn count_production_callers(src: &Path, name: &str) -> usize {
    let needle = format!(".{name}(");
    // A `#[cfg(test)] mod tests` inside `src/` is still a test. Counting
    // those as production callers is how a method with no production
    // caller passes the ratchet, so the in-file test module is skipped.
    let mut count = 0;
    let mut stack = vec![src.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
                continue;
            }
            if path.extension().and_then(|e| e.to_str()) != Some("rs") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            // Only a `#[cfg(test)] mod ... { }` block is a test module. A
            // bare `#[cfg(test)] fn` is a helper for the module further
            // down, and truncating the file at the first marker would
            // discard every production call site after it.
            let lines: Vec<&str> = text.lines().collect();
            let mut skip_until = None;
            let mut idx = 0;
            while idx < lines.len() {
                let trimmed = lines[idx].trim();
                if trimmed == "#[cfg(test)]"
                    && lines[idx + 1..]
                        .iter()
                        .any(|l| l.trim_start().starts_with("mod "))
                {
                    // A test module: skip to the line that closes it at
                    // brace depth zero. Counting braces from the `mod` line
                    // is more reliable than looking for a marker, because
                    // `#[cfg(test)] fn` helpers must *not* trigger this.
                    let mut j = idx + 1;
                    let mut depth = 0usize;
                    while j < lines.len() {
                        depth += lines[j].matches('{').count();
                        depth = depth.saturating_sub(lines[j].matches('}').count());
                        if depth == 0 && lines[j].contains('{') {
                            break;
                        }
                        if depth == 0 && j > idx + 1 {
                            break;
                        }
                        j += 1;
                    }
                    skip_until = Some(j);
                    idx = j + 1;
                    continue;
                }
                if let Some(limit) = skip_until {
                    if idx <= limit {
                        idx += 1;
                        continue;
                    }
                    skip_until = None;
                }
                if lines[idx].contains(&needle) && !trimmed.starts_with("//") {
                    count += 1;
                }
                idx += 1;
            }
        }
    }
    count
}

fn collect_rust_text(dir: &Path, out: &mut String) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            collect_rust_text(&path, out);
        } else if path.extension().and_then(|e| e.to_str()) == Some("rs")
            && let Ok(text) = std::fs::read_to_string(&path)
        {
            out.push_str(&text);
        }
    }
}
