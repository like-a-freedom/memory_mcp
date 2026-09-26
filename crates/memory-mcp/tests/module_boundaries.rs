use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

#[path = "support/module_boundary_checker.rs"]
mod module_boundary_checker;

use module_boundary_checker::{BoundaryChecker, ManifestRow, Violation};

fn repository_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .and_then(Path::parent)
        .expect("memory-mcp manifest must be nested under crates")
        .to_path_buf()
}

fn tracked_source_files(source_root: &Path) -> BTreeSet<PathBuf> {
    let mut pending = vec![source_root.to_path_buf()];
    let mut files = BTreeSet::new();

    while let Some(directory) = pending.pop() {
        let entries = fs::read_dir(&directory)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", directory.display()));
        for entry in entries {
            let path = entry
                .unwrap_or_else(|error| {
                    panic!("cannot inspect entry in {}: {error}", directory.display())
                })
                .path();
            if path.is_dir() {
                pending.push(path);
            } else if path.extension().and_then(|value| value.to_str()) == Some("rs") {
                files.insert(path);
            }
        }
    }

    files
}

fn source_key(path: &Path) -> String {
    path.strip_prefix(repository_root().join("crates/memory-mcp/src"))
        .expect("source must be below crates/memory-mcp/src")
        .to_string_lossy()
        .replace('\\', "/")
}

#[test]
fn migration_manifest_covers_every_tracked_source_file_exactly_once() {
    let root = repository_root();
    let manifest_path = root.join("docs/architecture/ddd-migration-manifest.csv");
    let rows = ManifestRow::read(&manifest_path);

    let actual = tracked_source_files(&root.join("crates/memory-mcp/src"))
        .iter()
        .map(|path| source_key(path))
        .collect::<BTreeSet<_>>();
    let mapped = rows
        .iter()
        .map(|row| row.source.clone())
        .collect::<BTreeSet<_>>();

    assert_eq!(
        actual, mapped,
        "manifest coverage differs from Rust sources"
    );
    assert_eq!(
        rows.len(),
        mapped.len(),
        "manifest contains duplicate sources"
    );
    for row in rows {
        assert!(
            !row.destination.is_empty(),
            "empty destination for {}",
            row.source
        );
        assert!(!row.owner.is_empty(), "empty owner for {}", row.source);
        assert!(!row.phase.is_empty(), "empty phase for {}", row.source);
        assert!(
            row.phase.starts_with("Phase "),
            "invalid phase for {}: {}",
            row.source,
            row.phase
        );
        assert!(
            !row.temporary_edge.is_empty(),
            "missing temporary-edge disposition for {}",
            row.source
        );
    }
}

#[test]
fn production_sources_have_no_unrecorded_boundary_violations() {
    let root = repository_root();
    let manifest_path = root.join("docs/architecture/ddd-migration-manifest.csv");
    let rows = ManifestRow::read(&manifest_path);
    let checker = BoundaryChecker::from_manifest(rows);
    let violations = checker.check_tree(&root.join("crates/memory-mcp/src"));

    assert!(
        violations.is_empty(),
        "unrecorded architecture violations:\n{}",
        format_violations(&violations)
    );
}

#[test]
fn checker_rejects_forbidden_dependencies_paths_reexports_and_cycles() {
    let checker = BoundaryChecker::from_manifest(Vec::new());
    let cases = [
        (
            "forbidden context dependency",
            "src/memory/infra/reader.rs",
            "use crate::identity::api::Account;",
        ),
        (
            "private layer path",
            "src/identity/infra/store.rs",
            "use crate::identity::domain::policy::can_delete;",
        ),
        (
            "infra re-export",
            "src/identity/mod.rs",
            "pub use crate::identity::infra::Store;",
        ),
        (
            "raw storage escape hatch",
            "src/operations/api.rs",
            "pub fn query(sql: &str, db: &DbClient);",
        ),
        (
            "context cycle",
            "src/knowledge/application/reconcile.rs",
            "use crate::memory::api::Episode;",
        ),
        // Grouped imports must not hide a forbidden dependency.
        (
            "grouped import hiding a forbidden dependency",
            "src/memory/infra/reader.rs",
            "use crate::{identity::api::Account, knowledge::api::Fact};",
        ),
        (
            "grouped import of a bare forbidden context",
            "src/embedding/infra/store.rs",
            "use crate::{embedding::api, identity};",
        ),
    ];

    for (label, path, source) in cases {
        let violations = checker.check_file(path, source);
        assert!(!violations.is_empty(), "checker accepted {label}");
    }

    // A grouped import that names only allowed or same-context
    // members must still pass, or the hardening would be too broad.
    for (label, path, source) in [
        (
            "grouped import of allowed dependencies",
            "src/memory/infra/reader.rs",
            "use crate::{knowledge::api::Fact, embedding::api::Vector};",
        ),
        (
            "grouped import of a same-context module",
            "src/memory/infra/reader.rs",
            "use crate::{memory::api::Recall, memory::ports::Port};",
        ),
    ] {
        let violations = checker.check_file(path, source);
        assert!(
            violations.is_empty(),
            "checker rejected a legitimate {label}: {}",
            format_violations(&violations)
        );
    }
}

#[test]
fn checker_accepts_documented_bootstrap_reexport_and_api_edges() {
    let checker = BoundaryChecker::from_manifest(Vec::new());
    let cases = [
        (
            "src/bootstrap/integration/tenancy_runtime.rs",
            "use crate::memory::api::Runtime;",
        ),
        (
            "src/operations/api.rs",
            "pub use crate::identity::api::AccountId;",
        ),
        (
            "src/identity/application/session.rs",
            "use crate::shared::error::MemoryError;",
        ),
        (
            "src/http/account.rs",
            "use crate::operations::api::begin_deletion;",
        ),
    ];

    for (path, source) in cases {
        let violations = checker.check_file(path, source);
        assert!(
            violations.is_empty(),
            "checker rejected allowed edge {path}: {}",
            format_violations(&violations)
        );
    }
}

#[test]
fn legacy_exceptions_are_exact_scoped_and_have_expiry() {
    let root = repository_root();
    let manifest_path = root.join("docs/architecture/ddd-migration-manifest.csv");
    let rows = ManifestRow::read(&manifest_path);

    for row in rows {
        if row.temporary_edge == "forbidden" {
            continue;
        }
        if let Some(legacy) = row.temporary_edge.strip_prefix("legacy:") {
            assert!(
                legacy.contains(" -> Phase "),
                "legacy exception for {} must name a removal phase",
                row.source
            );
            continue;
        }
        let edge = row
            .temporary_edge
            .strip_prefix("allowed:")
            .unwrap_or_else(|| {
                panic!(
                    "legacy exception for {} must be allowed, forbidden, or legacy",
                    row.source
                )
            });
        assert!(
            edge.starts_with("edge="),
            "legacy exception for {} must name an exact edge",
            row.source
        );
        assert!(
            edge.contains(" expiry=Phase "),
            "missing expiry for {}",
            row.source
        );
    }
}

fn format_violations(violations: &[Violation]) -> String {
    violations
        .iter()
        .map(|violation| format!("- {}: {}", violation.path, violation.rule))
        .collect::<Vec<_>>()
        .join("\n")
}

#[allow(dead_code)]
fn rules_by_path(rows: &[ManifestRow]) -> BTreeMap<&str, &str> {
    rows.iter()
        .map(|row| (row.source.as_str(), row.owner.as_str()))
        .collect()
}
