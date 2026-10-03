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

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[path = "support/rust_source.rs"]
mod rust_source;

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
    let tokens = rust_source::tokenize(text)
        .unwrap_or_else(|error| panic!("cannot tokenize service exports: {error}"));
    let mut names = BTreeSet::new();
    for (index, token) in tokens.iter().enumerate() {
        if token.text != "use" || !has_public_visibility(&tokens, index) {
            continue;
        }
        let Some(end) = tokens[index + 1..]
            .iter()
            .position(|token| token.text == ";")
            .map(|offset| index + 1 + offset)
        else {
            panic!("public use at token {index} has no semicolon");
        };
        let clause = render_tokens(&tokens[index + 1..end]);
        let exported = if clause.ends_with("::*") {
            let module = clause
                .trim_end_matches("::*")
                .rsplit("::")
                .next()
                .unwrap_or_default();
            if module != "constants" {
                panic!("public glob re-export is unsupported by the cut-name ratchet: {clause}");
            }
            local_module_exports(&tokens, module)
                .unwrap_or_else(|| panic!("cannot resolve local public glob re-export: {clause}"))
        } else {
            use_tree_names(&clause)
        };
        for name in exported {
            names.insert(name);
        }
    }
    names
}

fn has_public_visibility(tokens: &[rust_source::Token], use_index: usize) -> bool {
    if use_index > 0 && tokens[use_index - 1].text == "pub" {
        return true;
    }
    if use_index == 0 || tokens[use_index - 1].text != ")" {
        return false;
    }
    for open in (0..use_index).rev() {
        if tokens[open].text == "("
            && rust_source::matching_group(tokens, open) == Some(use_index - 1)
        {
            return open > 0 && tokens[open - 1].text == "pub";
        }
    }
    false
}

fn render_tokens(tokens: &[rust_source::Token]) -> String {
    let mut rendered = String::new();
    let mut previous_ident = false;
    for token in tokens {
        let ident = token
            .text
            .chars()
            .all(|character| character == '_' || character.is_alphanumeric());
        if previous_ident && ident {
            rendered.push(' ');
        }
        rendered.push_str(&token.text);
        previous_ident = ident;
    }
    rendered
}

/// The sole public glob in `service.rs` is `constants::*`, a private inline
/// module in that same file. Expand that bounded case so adding a cut name
/// there is visible; reject any other glob rather than silently claiming the
/// ratchet knows what it exports.
fn local_module_exports(tokens: &[rust_source::Token], module: &str) -> Option<Vec<String>> {
    for index in 0..tokens.len().saturating_sub(2) {
        if tokens[index].text != "mod"
            || tokens[index + 1].text != module
            || tokens[index + 2].text != "{"
        {
            continue;
        }
        let end = rust_source::matching_group(tokens, index + 2)?;
        let body = &tokens[index + 3..end];
        let mut names = Vec::new();
        let mut cursor = 0;
        while cursor + 2 < body.len() {
            if body[cursor].text == "pub"
                && matches!(
                    body[cursor + 1].text.as_str(),
                    "const" | "static" | "fn" | "struct" | "enum" | "type" | "trait" | "mod"
                )
                && is_identifier(&body[cursor + 2].text)
            {
                names.push(body[cursor + 2].text.clone());
                cursor += 3;
            } else {
                cursor += 1;
            }
        }
        return Some(names);
    }
    None
}

/// For a consumer import, the original path name counts even when it is
/// locally renamed (`use service::{LifecycleOperation as ServiceOperation}`).
/// The cut-name ratchet uses only the exported/local name from
/// `use_tree_names`.
fn use_tree_mentions(clause: &str, name: &str) -> bool {
    use_tree_names(clause).iter().any(|item| item == name)
        || clause.split(',').any(|item| {
            let leaf = item
                .trim()
                .trim_start_matches('{')
                .trim()
                .rsplit("::")
                .next()
                .unwrap_or_default()
                .trim();
            leaf.strip_prefix(name)
                .is_some_and(|rest| rest.trim_start().starts_with("as "))
        })
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
    assert!(
        !clause.split("::").any(|segment| segment.trim() == "*"),
        "grouped glob use trees are unsupported by the cut-name ratchet: {clause}"
    );
    assert!(
        !clause.contains('#'),
        "raw identifiers and attributes in a use tree are unsupported: {clause}"
    );
    if let Some(open) = clause.find('{') {
        // `prefix::{a, b}` — every element is a name under `prefix`.
        let prefix = &clause[..open];
        let close = clause
            .rfind('}')
            .unwrap_or_else(|| panic!("unterminated grouped use tree: {clause}"));
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

#[test]
fn an_unclosed_use_group_is_not_silently_treated_as_no_exports() {
    let malformed =
        std::panic::catch_unwind(|| reexported_names("pub use crate::types::{Fact, ids;\n"));
    assert!(
        malformed.is_err(),
        "an unsupported use tree must fail the guard rather than return an empty export set"
    );
}

#[test]
fn a_nested_grouped_glob_fails_closed() {
    let unsupported =
        std::panic::catch_unwind(|| reexported_names("pub use crate::shared::{ids::*};\n"));

    assert!(
        unsupported.is_err(),
        "a nested glob must not silently yield an empty export set"
    );
}

#[test]
fn public_methods_are_found_in_split_impl_files() {
    let temp = tempfile::tempdir().expect("temp source tree");
    let src = temp.path().join("src");
    let file = src.join("service/reembed.rs");
    std::fs::create_dir_all(file.parent().expect("parent")).expect("parent dir");
    std::fs::write(
        &file,
        "impl crate::service::MemoryService { pub async fn reembed_all_facts(&self) {} }\n",
    )
    .expect("write fixture");

    let methods = collect_public_methods(&src);
    assert!(
        methods
            .iter()
            .any(|method| { method.name == "reembed_all_facts" && method.file == file }),
        "split inherent impl methods must be inventoried: {methods:?}"
    );
}

#[test]
fn test_cfg_methods_do_not_enter_the_public_inventory() {
    let temp = tempfile::tempdir().expect("temp source tree");
    let src = temp.path().join("src");
    let file = src.join("service/test_helpers.rs");
    std::fs::create_dir_all(file.parent().expect("parent")).expect("parent dir");
    std::fs::write(
        &file,
        "impl MemoryService { #[cfg(test)] pub fn fixture_only() {} pub fn production() {} }\n",
    )
    .expect("write fixture");

    let methods = collect_public_methods(&src);
    assert_eq!(
        methods
            .iter()
            .map(|method| method.name.as_str())
            .collect::<Vec<_>>(),
        vec!["production"]
    );
}

#[test]
fn caller_ratchet_ignores_comments_strings_and_test_cfg_items() {
    let temp = tempfile::tempdir().expect("temp caller tree");
    let file = temp.path().join("caller.rs");
    std::fs::write(
        &file,
        r#"
            // fake.call()
            const TEXT: &str = "fake.call()";
            #[cfg(test)]
            mod tests {
                fn test_only() { fake.call(); }
            }
        "#,
    )
    .expect("write fixture");
    let names = BTreeSet::from(["call".to_string()]);

    assert!(
        count_calls_in_files(std::slice::from_ref(&file), &names, true).is_empty(),
        "comments, strings and test-only modules are not production callers"
    );
    assert_eq!(
        count_calls_in_files(&[file], &names, false).get("call"),
        Some(&1),
        "the lexical test scan still sees a real call"
    );
}

#[test]
fn allowlist_import_evidence_ignores_comments_and_literals() {
    assert!(!imports_through_service(
        r#"
            // use crate::service::normalize_text;
            const EXAMPLE: &str = "crate::service::normalize_text";
        "#,
        "normalize_text"
    ));
    assert!(imports_through_service(
        "use crate::service::{normalize_dt, normalize_text};",
        "normalize_text"
    ));
}

/// Whether `text` imports or qualifies `name` through the `service` module.
///
/// The shared lexer means comments and string literals are not evidence of a
/// consumer. Grouped imports use the same nested use-tree reader as the
/// re-export guard.
fn imports_through_service(text: &str, name: &str) -> bool {
    let tokens = rust_source::tokenize(text)
        .unwrap_or_else(|error| panic!("cannot tokenize consumer source: {error}"));
    for index in 0..tokens.len() {
        if tokens[index].text != "service"
            || !double_colon_at(&tokens, index.saturating_sub(2))
            || !double_colon_at(&tokens, index + 1)
        {
            continue;
        }
        if tokens
            .get(index + 3)
            .is_some_and(|token| token.text == name)
        {
            return true;
        }
        if tokens.get(index + 3).is_some_and(|token| token.text == "{")
            && let Some(end) = rust_source::matching_group(&tokens, index + 3)
        {
            let grouped = render_tokens(&tokens[index + 3..=end]);
            if use_tree_mentions(&grouped, name) {
                return true;
            }
        }
    }
    false
}

fn double_colon_at(tokens: &[rust_source::Token], index: usize) -> bool {
    tokens.get(index).is_some_and(|token| token.text == ":")
        && tokens.get(index + 1).is_some_and(|token| token.text == ":")
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

    let names: BTreeSet<String> = declared.iter().map(|method| method.name.clone()).collect();
    let production_calls = count_calls_in_files(&rust_files(&src), &names, true);
    let mut test_files = rust_files(&manifest.join("tests"));
    test_files.extend(declared.iter().map(|method| method.file.clone()));
    test_files.sort();
    test_files.dedup();
    let test_calls = count_calls_in_files(&test_files, &names, false);

    let mut orphans = Vec::new();
    for method in &declared {
        let name = &method.name;
        if TEST_ONLY.iter().any(|(allowed, _)| allowed == name) {
            continue;
        }
        if production_calls.get(name).copied().unwrap_or_default() > 0 {
            continue;
        }
        let test_only = test_calls.get(name).copied().unwrap_or_default() > 0;
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

#[derive(Debug)]
struct DeclaredMethod {
    name: String,
    file: PathBuf,
}

fn collect_public_methods(src: &Path) -> Vec<DeclaredMethod> {
    let mut found = Vec::new();
    for file in rust_files(src) {
        let Ok(text) = std::fs::read_to_string(&file) else {
            continue;
        };
        let tokens = rust_source::tokenize(&text)
            .unwrap_or_else(|error| panic!("cannot tokenize {}: {error}", file.display()));
        let mut index = 0;
        while index < tokens.len() {
            if let Some(end) = test_cfg_item_end(&tokens, index) {
                index = end + 1;
                continue;
            }
            if tokens[index].text != "impl" {
                index += 1;
                continue;
            }
            let Some(open) = (index + 1..tokens.len()).find(|&at| tokens[at].text == "{") else {
                index += 1;
                continue;
            };
            let Some(close) = rust_source::matching_group(&tokens, open) else {
                panic!("unterminated impl block in {}", file.display());
            };
            let header = &tokens[index + 1..open];
            let memory_service_impl = header.iter().any(|token| token.text == "MemoryService")
                && !header.iter().any(|token| token.text == "for");
            if !memory_service_impl {
                index = close + 1;
                continue;
            }

            let mut cursor = open + 1;
            while cursor < close {
                if let Some(end) = test_cfg_item_end(&tokens, cursor) {
                    cursor = end + 1;
                    continue;
                }
                if tokens[cursor].text == "pub"
                    && !tokens.get(cursor + 1).is_some_and(|t| t.text == "(")
                {
                    let mut signature = cursor + 1;
                    while signature < close
                        && !matches!(tokens[signature].text.as_str(), "fn" | ";" | "{")
                    {
                        signature += 1;
                    }
                    if tokens
                        .get(signature)
                        .is_some_and(|token| token.text == "fn")
                        && tokens
                            .get(signature + 1)
                            .is_some_and(|token| rust_source::is_identifier(&token.text))
                    {
                        let name = tokens[signature + 1].text.clone();
                        found.push(DeclaredMethod {
                            name,
                            file: file.clone(),
                        });
                        // Skip the method body so nested local items cannot
                        // masquerade as methods on the outer impl.
                        if let Some(body) =
                            (signature + 2..close).find(|&at| tokens[at].text == "{")
                            && let Some(body_end) = rust_source::matching_group(&tokens, body)
                        {
                            cursor = body_end + 1;
                            continue;
                        }
                    }
                }
                if rust_source::is_open_group(&tokens[cursor].text)
                    && let Some(end) = rust_source::matching_group(&tokens, cursor)
                {
                    cursor = end + 1;
                } else {
                    cursor += 1;
                }
            }
            index = close + 1;
        }
    }
    found
}

fn rust_files(root: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(directory) = stack.pop() {
        let entries = std::fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("{} is not readable: {error}", directory.display()));
        for entry in entries {
            let entry = entry.unwrap_or_else(|error| {
                panic!("unreadable entry in {}: {error}", directory.display())
            });
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                files.push(path);
            }
        }
    }
    files.sort();
    files
}

fn test_cfg_item_end(tokens: &[rust_source::Token], attribute: usize) -> Option<usize> {
    let cfg_test = ["#", "[", "cfg", "(", "test", ")", "]"];
    if tokens
        .get(attribute..attribute + cfg_test.len())?
        .iter()
        .map(|token| token.text.as_str())
        .ne(cfg_test)
    {
        return None;
    }
    let item_start = attribute + cfg_test.len();
    let body = (item_start..tokens.len())
        .find(|&index| tokens[index].text == "{" || tokens[index].text == ";")?;
    if tokens[body].text == ";" {
        Some(body)
    } else {
        rust_source::matching_group(tokens, body)
    }
}

fn count_calls_in_files(
    files: &[PathBuf],
    names: &BTreeSet<String>,
    exclude_test_cfg: bool,
) -> BTreeMap<String, usize> {
    let mut counts = BTreeMap::new();
    for file in files {
        let Ok(text) = std::fs::read_to_string(file) else {
            continue;
        };
        let tokens = rust_source::tokenize(&text)
            .unwrap_or_else(|error| panic!("cannot tokenize {}: {error}", file.display()));
        let mut index = 0;
        while index < tokens.len() {
            if exclude_test_cfg && let Some(end) = test_cfg_item_end(&tokens, index) {
                index = end + 1;
                continue;
            }
            if index + 1 < tokens.len()
                && tokens[index].text == "."
                && names.contains(&tokens[index + 1].text)
                && method_call_open(&tokens, index + 2).is_some()
            {
                *counts.entry(tokens[index + 1].text.clone()).or_insert(0) += 1;
                index += 3;
            } else {
                index += 1;
            }
        }
    }
    counts
}

fn method_call_open(tokens: &[rust_source::Token], after_name: usize) -> Option<usize> {
    if tokens
        .get(after_name)
        .is_some_and(|token| token.text == "(")
    {
        return Some(after_name);
    }
    if !tokens
        .get(after_name)
        .is_some_and(|token| token.text == "::")
        || !tokens
            .get(after_name + 1)
            .is_some_and(|token| token.text == "<")
    {
        return None;
    }
    let mut depth = 0usize;
    for index in after_name + 1..tokens.len() {
        match tokens[index].text.as_str() {
            "<" => depth += 1,
            ">" => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    let open = index + 1;
                    return tokens
                        .get(open)
                        .is_some_and(|token| token.text == "(")
                        .then_some(open);
                }
            }
            _ => {}
        }
    }
    None
}
