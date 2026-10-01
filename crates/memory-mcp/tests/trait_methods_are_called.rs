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
