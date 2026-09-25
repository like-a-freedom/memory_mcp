use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ManifestRow {
    pub(crate) source: String,
    pub(crate) destination: String,
    pub(crate) layer: String,
    pub(crate) owner: String,
    pub(crate) canonical_operations: String,
    pub(crate) phase: String,
    pub(crate) consumers: String,
    pub(crate) temporary_edge: String,
}

impl ManifestRow {
    pub(crate) fn read(path: &Path) -> Vec<Self> {
        let contents = fs::read_to_string(path).unwrap_or_else(|error| {
            panic!("cannot read migration manifest {}: {error}", path.display())
        });
        let mut lines = contents.lines();
        let header = lines
            .next()
            .expect("migration manifest must contain a header");
        assert_eq!(
            header,
            "source,destination,layer,owner,canonical_operations,phase,consumers,temporary_edge",
            "unexpected migration manifest header"
        );

        lines
            .filter(|line| !line.trim().is_empty())
            .enumerate()
            .map(|(index, line)| {
                let fields = parse_csv_line(line);
                assert_eq!(
                    fields.len(),
                    8,
                    "manifest line {} must have eight fields",
                    index + 2
                );
                Self {
                    source: fields[0].to_string(),
                    destination: fields[1].to_string(),
                    layer: fields[2].to_string(),
                    owner: fields[3].to_string(),
                    canonical_operations: fields[4].to_string(),
                    phase: fields[5].to_string(),
                    consumers: fields[6].to_string(),
                    temporary_edge: fields[7].to_string(),
                }
            })
            .collect()
    }
}

fn parse_csv_line(line: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut field = String::new();
    let mut quoted = false;
    let mut characters = line.chars().peekable();

    while let Some(character) = characters.next() {
        match character {
            '"' if quoted && characters.peek() == Some(&'"') => {
                field.push('"');
                characters.next();
            }
            '"' => quoted = !quoted,
            ',' if !quoted => {
                fields.push(std::mem::take(&mut field));
            }
            character => field.push(character),
        }
    }

    fields.push(field);
    fields
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub(crate) struct Violation {
    pub(crate) path: String,
    pub(crate) rule: String,
}

pub(crate) struct BoundaryChecker {
    allowed_temporary_edges: BTreeSet<String>,
}

impl BoundaryChecker {
    pub(crate) fn from_manifest(rows: Vec<ManifestRow>) -> Self {
        Self {
            allowed_temporary_edges: rows
                .into_iter()
                .filter_map(|row| {
                    let exception = row.temporary_edge.strip_prefix("allowed:")?;
                    exception.strip_prefix("edge=").map(ToString::to_string)
                })
                .collect(),
        }
    }

    pub(crate) fn check_tree(&self, source_root: &Path) -> Vec<Violation> {
        let mut files = Vec::new();
        self.collect_rust_files(source_root, &mut files);
        files.sort();

        let mut violations = Vec::new();
        let mut graph = BTreeMap::<String, BTreeSet<String>>::new();

        for path in &files {
            let relative = path
                .strip_prefix(source_root)
                .expect("collected path must be below source root")
                .to_string_lossy()
                .replace('\\', "/");
            let source = fs::read_to_string(path)
                .unwrap_or_else(|error| panic!("cannot read {}: {error}", path.display()));
            let logical_path = format!("src/{relative}");
            let file_violations = self.check_file(&logical_path, &source);
            if let Some(context) = context_name(&logical_path) {
                graph
                    .entry(context.to_string())
                    .or_default()
                    .extend(business_dependencies(context, &source));
            }
            violations.extend(file_violations);
        }

        violations.extend(self.check_cycles(&graph));
        violations
    }

    pub(crate) fn check_file(&self, path: &str, source: &str) -> Vec<Violation> {
        let mut violations = Vec::new();
        let context = context_name(path);
        let layer = layer_name(path);

        if let Some(context) = context {
            for dependency in business_dependencies(context, source) {
                if !allowed_business_dependency(context, &dependency)
                    && !self.allowed_temporary_edges.contains(&dependency)
                {
                    violations.push(violation(
                        path,
                        format!("{context} cannot depend on {dependency}"),
                    ));
                }
            }

            if layer == Some("domain")
                && contains_any(
                    source,
                    &[
                        "crate::http",
                        "crate::storage",
                        "crate::platform",
                        "crate::bootstrap",
                        "std::env",
                        "reqwest::",
                        "surreal",
                    ],
                )
            {
                violations.push(violation(path, "domain depends on an outward layer"));
            }

            if layer == Some("application")
                && contains_any(
                    source,
                    &[
                        "crate::http",
                        "crate::storage",
                        "crate::bootstrap",
                        "surreal",
                    ],
                )
            {
                violations.push(violation(path, "application depends on an outward layer"));
            }

            if layer == Some("api")
                && contains_any(
                    source,
                    &[
                        "crate::identity::infra",
                        "crate::tenancy::infra",
                        "crate::provisioning::infra",
                        "crate::operations::infra",
                        "crate::memory::infra",
                        "crate::knowledge::infra",
                        "crate::embedding::infra",
                    ],
                )
            {
                violations.push(violation(
                    path,
                    "API re-exports or imports concrete infrastructure",
                ));
            }
        }

        if path.ends_with("/mod.rs")
            && contains_any(
                source,
                &[
                    "pub use crate::identity::infra",
                    "pub use crate::tenancy::infra",
                    "pub use crate::provisioning::infra",
                    "pub use crate::operations::infra",
                    "pub use crate::memory::infra",
                    "pub use crate::knowledge::infra",
                    "pub use crate::embedding::infra",
                ],
            )
        {
            violations.push(violation(
                path,
                "module root re-exports concrete infrastructure",
            ));
        }

        if let Some(context) = context {
            let forbidden_layer = layer_name_for_import(source, &format!("src/{context}"));
            if let Some(forbidden_layer) = forbidden_layer {
                violations.push(violation(
                    path,
                    format!("{context} consumer imports private {forbidden_layer} path"),
                ));
            }
        }

        if source.contains("pub fn query(") && source.contains("DbClient") {
            violations.push(violation(path, "application interface exposes raw storage"));
        }

        if path.ends_with("/mod.rs") && source.contains("pub mod domain")
            || path.ends_with("/mod.rs") && source.contains("pub mod application")
            || path.ends_with("/mod.rs") && source.contains("pub mod infra")
        {
            violations.push(violation(
                path,
                "module root publicly exposes an internal layer",
            ));
        }

        violations
    }

    fn collect_rust_files(&self, directory: &Path, files: &mut Vec<std::path::PathBuf>) {
        for entry in fs::read_dir(directory)
            .unwrap_or_else(|error| panic!("cannot read {}: {error}", directory.display()))
        {
            let path = entry
                .unwrap_or_else(|error| {
                    panic!("cannot inspect entry in {}: {error}", directory.display())
                })
                .path();
            if path.is_dir() {
                self.collect_rust_files(&path, files);
            } else if path.extension().and_then(|value| value.to_str()) == Some("rs") {
                files.push(path);
            }
        }
    }

    fn check_cycles(&self, graph: &BTreeMap<String, BTreeSet<String>>) -> Vec<Violation> {
        let mut violations = BTreeSet::new();
        let nodes = graph.keys().cloned().collect::<Vec<_>>();

        for node in nodes {
            if graph
                .get(&node)
                .is_some_and(|dependencies| dependencies.contains(&node))
            {
                violations.insert(violation(
                    &node,
                    "business dependency cycle includes self-edge",
                ));
            }
            let mut visited = BTreeSet::new();
            let mut active = BTreeSet::new();
            if let Some(cycle) = find_cycle(&node, graph, &mut visited, &mut active) {
                let cycle = cycle.join(" -> ");
                violations.insert(violation(
                    &node,
                    format!("business dependency cycle: {cycle}"),
                ));
            }
        }

        violations.into_iter().collect()
    }
}

fn violation(path: &str, rule: impl Into<String>) -> Violation {
    Violation {
        path: path.to_string(),
        rule: rule.into(),
    }
}

fn context_name(path: &str) -> Option<&str> {
    path.strip_prefix("src/")
        .and_then(|path| path.split('/').next())
        .filter(|name| {
            matches!(
                *name,
                "identity"
                    | "tenancy"
                    | "provisioning"
                    | "operations"
                    | "memory"
                    | "knowledge"
                    | "embedding"
            )
        })
}

fn layer_name(path: &str) -> Option<&str> {
    path.split('/').find(|segment| {
        matches!(
            *segment,
            "api" | "domain" | "application" | "infra" | "integration"
        )
    })
}

fn business_dependencies(context: &str, source: &str) -> BTreeSet<String> {
    const CONTEXTS: [&str; 7] = [
        "identity",
        "tenancy",
        "provisioning",
        "operations",
        "memory",
        "knowledge",
        "embedding",
    ];

    CONTEXTS
        .into_iter()
        .filter(|dependency| {
            *dependency != context
                && (source.contains(&format!("crate::{dependency}::"))
                    || source.contains(&format!("super::{dependency}::")))
        })
        .map(ToString::to_string)
        .collect()
}

fn allowed_business_dependency(context: &str, dependency: &str) -> bool {
    matches!(
        (context, dependency),
        ("operations", "identity")
            | ("operations", "tenancy")
            | ("operations", "provisioning")
            | ("provisioning", "identity")
            | ("provisioning", "tenancy")
            | ("provisioning", "memory")
            | ("provisioning", "knowledge")
            | ("memory", "knowledge")
            | ("memory", "embedding")
            | ("knowledge", "embedding")
    )
}

fn layer_name_for_import(source: &str, module_root: &str) -> Option<&'static str> {
    let module_root = module_root.strip_prefix("src/")?;

    ["domain", "application", "infra"]
        .into_iter()
        .find(|layer| {
            source.contains(&format!("crate::{module_root}::{layer}::"))
                || source.contains(&format!("super::{layer}::"))
        })
        .map(|layer| match layer {
            "domain" => "domain",
            "application" => "application",
            "infra" => "infra",
            _ => unreachable!(),
        })
}

fn find_cycle(
    node: &str,
    graph: &BTreeMap<String, BTreeSet<String>>,
    visited: &mut BTreeSet<String>,
    active: &mut BTreeSet<String>,
) -> Option<Vec<String>> {
    if !active.insert(node.to_string()) {
        return Some(vec![node.to_string()]);
    }
    if !visited.insert(node.to_string()) {
        active.remove(node);
        return None;
    }

    if let Some(dependencies) = graph.get(node) {
        for dependency in dependencies {
            if graph.contains_key(dependency) {
                if let Some(mut cycle) = find_cycle(dependency, graph, visited, active) {
                    cycle.insert(0, node.to_string());
                    active.remove(node);
                    return Some(cycle);
                }
            } else {
                active.remove(node);
                return Some(vec![node.to_string(), dependency.clone()]);
            }
        }
    }

    active.remove(node);
    None
}

fn contains_any(source: &str, needles: &[&str]) -> bool {
    needles.iter().any(|needle| source.contains(needle))
}
