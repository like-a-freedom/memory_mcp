//! Every listed store-trait method must have lexical evidence of a use site.
//!
//! A trait method is a promise. `TaskStore::update_progress_fenced` promised
//! that progress could be reported under a fence, had a real implementation in
//! the worker store, and was called from nowhere — not from production, not
//! from a test, not from the test driver. Nothing reports that: rustc emits
//! no warning for a trait method that is implemented but never called, and
//! `#[async_trait]` hides even the implementation.
//
//! The exact named trait body is checked for each declaration, and the bounded
//! source lexer excludes comments and literals from caller evidence. Receiver
//! type resolution is not possible here: `.load()` is lexical evidence, not a
//! proof that the receiver implements `TaskStore`. Aliases, macros and blanket
//! implementations are outside this ratchet's guarantee.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};

#[path = "support/rust_source.rs"]
mod rust_source;

/// `(trait path, method name)` for every method that must have a caller.
///
/// Adding a row is a claim that the method is used. Deleting a row without
/// deleting the method makes this test stop covering it, so the table and the
/// trait have to be changed together.
const METHODS: &[(&str, &str)] = &[
    // Every method of every store trait in the crate, extracted from the trait
    // bodies rather than listed from memory: 79 methods across 11 traits. A
    // spot check of 13 would have left 66 uncovered, and a table that claims
    // more coverage than it has is worse than no table at all.
    //
    // Adding a method to a trait means adding a row here, and
    // `every_trait_method_named_in_the_table_still_exists` is what catches it
    // in the other direction: a row cannot outlive the method it names.
    (
        "http/registry/storage.rs",
        "AccountStore::find_account_by_id",
    ),
    (
        "http/registry/storage.rs",
        "AccountStore::find_account_by_identity",
    ),
    ("http/registry/storage.rs", "AccountStore::write_account"),
    (
        "http/registry/storage.rs",
        "AccountStore::transition_account_state",
    ),
    (
        "http/registry/storage.rs",
        "AccountStore::create_account_bundle",
    ),
    (
        "http/registry/storage.rs",
        "AccountStore::create_oidc_account_bundle",
    ),
    (
        "http/registry/storage.rs",
        "AccountStore::begin_account_deletion",
    ),
    (
        "http/registry/storage.rs",
        "IdentityStore::find_external_identities",
    ),
    (
        "http/registry/storage.rs",
        "IdentityStore::link_external_identity",
    ),
    (
        "http/registry/storage.rs",
        "IdentityStore::unlink_external_identity",
    ),
    (
        "http/registry/storage.rs",
        "IdentityStore::replace_external_identity",
    ),
    (
        "http/registry/storage.rs",
        "TenantStore::find_tenant_by_account",
    ),
    ("http/registry/storage.rs", "TenantStore::find_tenant_by_id"),
    ("http/registry/storage.rs", "TenantStore::write_tenant"),
    (
        "http/registry/storage.rs",
        "TenantStore::update_tenant_state",
    ),
    (
        "http/registry/storage.rs",
        "TenantStore::update_tenant_state_fenced",
    ),
    (
        "http/registry/storage.rs",
        "TenantStore::update_tenant_schema_version_fenced",
    ),
    ("http/registry/storage.rs", "TenantStore::list_tenants"),
    (
        "http/registry/storage.rs",
        "TenantStore::list_ready_tenants",
    ),
    (
        "http/registry/storage.rs",
        "TenantStore::list_deleting_tenants",
    ),
    (
        "http/registry/storage.rs",
        "TenantStore::begin_operator_deletion",
    ),
    (
        "http/registry/storage.rs",
        "TenantStore::finalize_account_deletion",
    ),
    ("http/registry/storage.rs", "ApiKeyStore::find_api_key"),
    ("http/registry/storage.rs", "ApiKeyStore::write_api_key"),
    ("http/registry/storage.rs", "ApiKeyStore::list_api_keys"),
    ("http/registry/storage.rs", "ApiKeyStore::revoke_api_key"),
    ("http/registry/storage.rs", "ApiKeyStore::touch_api_key"),
    (
        "http/registry/storage.rs",
        "ApiKeyStore::create_api_key_if_below_limit",
    ),
    (
        "http/registry/storage.rs",
        "ApiKeyStore::revoke_all_api_keys",
    ),
    (
        "http/registry/storage.rs",
        "ProvisioningStore::claim_provisioning",
    ),
    (
        "http/registry/storage.rs",
        "ProvisioningStore::release_provisioning_lease",
    ),
    (
        "http/registry/storage.rs",
        "ProvisioningStore::heartbeat_provisioning",
    ),
    (
        "http/registry/storage.rs",
        "ProvisioningStore::list_due_provisioning",
    ),
    (
        "http/registry/storage.rs",
        "ProvisioningStore::append_provisioning_event",
    ),
    ("http/registry/storage.rs", "UsageStore::load_plan"),
    ("http/registry/storage.rs", "UsageStore::ensure_plan"),
    ("http/registry/storage.rs", "UsageStore::load_usage"),
    (
        "http/registry/storage.rs",
        "UsageStore::reserve_ingest_usage",
    ),
    ("http/registry/storage.rs", "UsageStore::reconcile_usage"),
    ("http/registry/storage.rs", "UsageStore::ensure_local_plan"),
    ("http/registry/storage.rs", "SessionStore::store_session"),
    ("http/registry/storage.rs", "SessionStore::find_session"),
    ("http/registry/storage.rs", "SessionStore::touch_session"),
    ("http/registry/storage.rs", "SessionStore::delete_session"),
    (
        "http/registry/storage.rs",
        "SessionStore::store_oidc_request",
    ),
    (
        "http/registry/storage.rs",
        "SessionStore::take_oidc_request",
    ),
    (
        "http/registry/storage.rs",
        "SessionStore::create_deletion_challenge",
    ),
    (
        "http/registry/storage.rs",
        "SessionStore::consume_deletion_challenge",
    ),
    (
        "http/registry/storage.rs",
        "BrowserPolicyStore::reconcile_browser_policy",
    ),
    (
        "http/registry/storage.rs",
        "BrowserPolicyStore::remove_browser_auth_method",
    ),
    ("http/registry/storage.rs", "StoreHealth::ping"),
    (
        "service/local_admin/contracts.rs",
        "LocalAdminStore::issue_challenge",
    ),
    (
        "service/local_admin/contracts.rs",
        "LocalAdminStore::inspect_challenge",
    ),
    (
        "service/local_admin/contracts.rs",
        "LocalAdminStore::finish_challenge",
    ),
    (
        "service/local_admin/contracts.rs",
        "LocalAdminStore::credential",
    ),
    (
        "service/local_admin/contracts.rs",
        "LocalAdminStore::open_session",
    ),
    (
        "service/local_admin/contracts.rs",
        "LocalAdminStore::resolve_session",
    ),
    (
        "service/local_admin/contracts.rs",
        "LocalAdminStore::rotate_session",
    ),
    (
        "service/local_admin/contracts.rs",
        "LocalAdminStore::revoke_session",
    ),
    (
        "service/local_admin/contracts.rs",
        "LocalAdminStore::reserve_attempt",
    ),
    (
        "service/local_admin/contracts.rs",
        "LocalAdminStore::cleanup_rate_buckets",
    ),
    (
        "service/local_admin/contracts.rs",
        "LocalAdminStore::record_failure",
    ),
    (
        "service/local_admin/contracts.rs",
        "LocalAdminStore::create_client",
    ),
    (
        "service/local_admin/contracts.rs",
        "LocalAdminStore::list_clients",
    ),
    (
        "service/local_admin/contracts.rs",
        "LocalAdminStore::client",
    ),
    (
        "service/local_admin/contracts.rs",
        "LocalAdminStore::list_client_keys",
    ),
    (
        "service/local_admin/contracts.rs",
        "LocalAdminStore::insert_client_key",
    ),
    (
        "service/local_admin/contracts.rs",
        "LocalAdminStore::revoke_client_key",
    ),
    (
        "service/local_admin/contracts.rs",
        "LocalAdminStore::set_client_state",
    ),
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

fn relative(path: &Path) -> String {
    path.strip_prefix(Path::new(env!("CARGO_MANIFEST_DIR")))
        .unwrap_or(path)
        .display()
        .to_string()
}

/// This is intentionally a *lexical usage ratchet*, not a type-resolved proof
/// that the receiver implements the named trait. It excludes comments and
/// literals and requires the named trait declaration to contain the method,
/// but it cannot resolve receiver types, aliases, macros or blanket impls.
fn has_caller(sources: &BTreeSet<(String, String)>, qualified: &str) -> Option<String> {
    let (trait_name, method) = qualified.split_once("::")?;
    for (file, text) in sources {
        let tokens = rust_source::tokenize(text)
            .unwrap_or_else(|error| panic!("cannot tokenize {file}: {error}"));
        if contains_method_call(&tokens, trait_name, method) {
            return Some(file.clone());
        }
    }
    None
}

fn contains_method_call(tokens: &[rust_source::Token], trait_name: &str, method: &str) -> bool {
    debug_assert!(rust_source::is_identifier(trait_name));
    debug_assert!(rust_source::is_identifier(method));
    for index in 0..tokens.len() {
        // Receiver call: `receiver.method(...)`, with whitespace irrelevant.
        if tokens[index].text == "."
            && tokens
                .get(index + 1)
                .is_some_and(|token| token.text == method)
            && (tokens.get(index + 2).is_some_and(|token| token.text == "(")
                || turbofish_call_follows(tokens, index + 2))
        {
            return true;
        }

        // Explicit path call: `Trait::method(...)` or
        // `<T as Trait>::method(...)`. This is stronger evidence for a
        // particular trait, but receiver calls remain lexical only.
        if tokens[index].text == trait_name
            && double_colon_at(tokens, index + 1)
            && tokens
                .get(index + 3)
                .is_some_and(|token| token.text == method)
            && (tokens.get(index + 4).is_some_and(|token| token.text == "(")
                || turbofish_call_follows(tokens, index + 4))
        {
            return true;
        }
        if tokens[index].text == trait_name
            && tokens.get(index + 1).is_some_and(|token| token.text == ">")
            && double_colon_at(tokens, index + 2)
            && tokens
                .get(index + 4)
                .is_some_and(|token| token.text == method)
            && (tokens.get(index + 5).is_some_and(|token| token.text == "(")
                || turbofish_call_follows(tokens, index + 5))
        {
            return true;
        }
    }
    false
}

fn turbofish_call_follows(tokens: &[rust_source::Token], start: usize) -> bool {
    rust_source::call_open(tokens, start).is_some()
}

fn double_colon_at(tokens: &[rust_source::Token], index: usize) -> bool {
    tokens.get(index).is_some_and(|token| token.text == ":")
        && tokens.get(index + 1).is_some_and(|token| token.text == ":")
}

fn trait_declares_method(text: &str, trait_name: &str, method: &str) -> bool {
    let tokens = rust_source::tokenize(text)
        .unwrap_or_else(|error| panic!("cannot tokenize trait declaration: {error}"));
    for index in 0..tokens.len() {
        if tokens[index].text != "trait"
            || !tokens
                .get(index + 1)
                .is_some_and(|token| token.text == trait_name)
        {
            continue;
        }
        let Some(open) = (index + 2..tokens.len()).find(|&at| tokens[at].text == "{") else {
            continue;
        };
        let Some(close) = rust_source::matching_group(&tokens, open) else {
            continue;
        };
        let mut nested = 0usize;
        let mut cursor = open + 1;
        while cursor < close {
            match tokens[cursor].text.as_str() {
                group if rust_source::is_open_group(group) => {
                    if let Some(end) = rust_source::matching_group(&tokens, cursor) {
                        nested += 1;
                        cursor = end + 1;
                        nested -= 1;
                        continue;
                    }
                }
                "fn" if nested == 0
                    && tokens
                        .get(cursor + 1)
                        .is_some_and(|token| token.text == method) =>
                {
                    return true;
                }
                _ => {}
            }
            cursor += 1;
        }
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
        let Some((trait_name, method)) = qualified.split_once("::") else {
            problems.push(format!("{qualified}: expected Trait::method spelling"));
            continue;
        };
        if !trait_declares_method(&text, trait_name, method) {
            problems.push(format!(
                "{qualified} is not declared in the named trait body in {path}. \
                 Delete its row from this table in the same commit, or the method \
                 is no longer covered."
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
        if has_caller(&sources, qualified).is_none() {
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

#[test]
fn trait_inventory_requires_the_named_trait_body() {
    let source = r#"
        fn load() {}
        trait OtherStore { fn load(&self); }
        trait TaskStore { fn save(&self); }
    "#;
    assert!(!trait_declares_method(source, "TaskStore", "load"));
    assert!(trait_declares_method(source, "TaskStore", "save"));
    assert!(trait_declares_method(source, "OtherStore", "load"));
}

#[test]
fn lexical_call_scan_ignores_comments_and_string_literals() {
    let source = r#"
        // store.load()
        const EXAMPLE: &str = "store.load()";
        /* store.load() */
        fn caller(store: &impl TaskStore) { store.save(); }
    "#;
    let tokens = rust_source::tokenize(source).expect("tokenize");
    assert!(!contains_method_call(&tokens, "TaskStore", "load"));
    assert!(contains_method_call(&tokens, "TaskStore", "save"));
}

#[test]
fn turbofish_calls_are_found_for_receiver_associated_and_qualified_paths() {
    let sources = [
        "fn caller(store: Store) { store.load::<u8>(); }",
        "fn caller() { TaskStore::load::<u8>(); }",
        "fn caller() { <Store as TaskStore>::load::<u8>(); }",
    ];

    for source in sources {
        let tokens = rust_source::tokenize(source).expect("tokenize turbofish call");
        assert!(
            tokens
                .windows(2)
                .any(|pair| pair[0].text == ":" && pair[1].text == ":"),
            "the fixture uses the lexer's two-token `::` representation"
        );
        assert!(
            contains_method_call(&tokens, "TaskStore", "load"),
            "missed turbofish call in {source}"
        );
    }
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
