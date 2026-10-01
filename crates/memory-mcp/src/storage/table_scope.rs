//! Which context owns which table, and the only way to name one.
//!
//! `storage` connects to a database. It does not know what a `claim` is, and
//! the old allowlist said otherwise in the only way it could: a central list
//! of ten table names inside the module that executes SQL. It named ten of
//! the twenty-three the migrations create, so a caller that added a
//! `triple` query and forgot the list met `ConfigInvalid` — a configuration
//! error, for what is really a missing entry in a list nobody reads.
//!
//! The fix is not a longer list. It is that a table name can only be produced
//! by the bounded context that owns it, so a new table is claimed by adding it
//! where it is used and the compiler points at the one place that needed to
//! learn about it. `every_expected_schema_table_has_exactly_one_owner` in
//! `tests/typed_record_accessors.rs` is what holds the partition honest: every
//! table the schema creates is owned exactly once.

/// A table name released by its owning bounded context.
///
/// The field is `pub(crate)`, so nothing outside this crate can build one by
/// field access. The public constructor is
/// [`ReleaseOwnedTable::table`], an associated function on the context's own
/// `TableOwner` impl, and it carries a `debug_assert` that the context
/// actually declared the table.
///
/// That assertion is not the invariant — it is a development aid. In a release
/// build it is elided, and the real invariant is the equality test. The
/// alternative, a `pub const fn new(name)`, would validate nothing: a `const
/// fn` cannot look anything up, so it would be a newtype around a `&'static
/// str` anyone can spell, which is the allowlist problem in a type-safe
/// costume.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct OwnedTable(pub(crate) &'static str);

impl OwnedTable {
    /// The table name, for logging and for a test that asserts which table a
    /// store was asked for.
    ///
    /// Public because a name that cannot be read cannot be checked: the whole
    /// point of the type is that a caller can say *which* table it reached for.
    #[must_use]
    pub const fn as_str(&self) -> &'static str {
        self.0
    }
}

impl std::fmt::Display for OwnedTable {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.0)
    }
}

/// A bounded context's claim on its tables.
///
/// The partition is asserted against the migration list, so a table cannot be
/// added to one place and forgotten in the other.
pub trait TableOwner {
    /// Every table this context owns.
    const OWNED_TABLES: &'static [&'static str];
}

/// Release one of this context's tables.
///
/// Declared as an associated function rather than a free `OwnedTable::new` so
/// the only way to build the value is through a type that has declared the
/// table in `OWNED_TABLES`.
pub trait ReleaseOwnedTable: TableOwner {
    fn table(name: &'static str) -> OwnedTable;
}

// ---------------------------------------------------------------------------
// The partition.
//
// Grouped by the context that owns the concept, not by the order the tables
// appear in a migration. Each impl is three lines, and the `debug_assert` in
// the generated body is the whole enforcement.
// ---------------------------------------------------------------------------

/// The knowledge context's tables: entities, facts, edges, communities,
/// claims, triples and the projection tables derived from them.
#[derive(Debug, Clone, Copy)]
pub struct KnowledgeTables;

impl TableOwner for KnowledgeTables {
    const OWNED_TABLES: &'static [&'static str] = &[
        "entity",
        "fact",
        "edge",
        "community",
        "claim",
        "claim_relation",
        "claim_job",
        "claim_key_alias",
        "claim_policy",
        "triple",
        "entity_extraction_projection",
        "event_projection_job",
        "procedure_candidate",
        "memory_capture_audit",
    ];
}

impl ReleaseOwnedTable for KnowledgeTables {
    fn table(name: &'static str) -> OwnedTable {
        debug_assert!(
            Self::OWNED_TABLES.contains(&name),
            "knowledge reached for a table it does not own: {name}"
        );
        OwnedTable(name)
    }
}

/// The memory context's tables: episodes, inbox revisions and the memory
/// event stream.
#[derive(Debug, Clone, Copy)]
pub struct MemoryTables;

impl TableOwner for MemoryTables {
    const OWNED_TABLES: &'static [&'static str] = &["episode", "inbox_revision", "memory_event"];
}

impl ReleaseOwnedTable for MemoryTables {
    fn table(name: &'static str) -> OwnedTable {
        debug_assert!(
            Self::OWNED_TABLES.contains(&name),
            "memory reached for a table it does not own: {name}"
        );
        OwnedTable(name)
    }
}

/// The embedding context's tables.
#[derive(Debug, Clone, Copy)]
pub struct EmbeddingTables;

impl TableOwner for EmbeddingTables {
    const OWNED_TABLES: &'static [&'static str] = &["embedding_state", "embedding_job"];
}

impl ReleaseOwnedTable for EmbeddingTables {
    fn table(name: &'static str) -> OwnedTable {
        debug_assert!(
            Self::OWNED_TABLES.contains(&name),
            "embedding reached for a table it does not own: {name}"
        );
        OwnedTable(name)
    }
}

/// The platform's own tables: its access log, its migration bookkeeping and
/// its durable task queue.
///
/// `storage` is not a bounded context, but it owns these three — they are its
/// bookkeeping, not anyone's domain — so it declares them through the same
/// trait rather than through the allowlist it replaces.
#[derive(Debug, Clone, Copy)]
pub struct PlatformTables;

impl TableOwner for PlatformTables {
    const OWNED_TABLES: &'static [&'static str] =
        &["event_log", "query_log", "script_migration", "task"];
}

impl ReleaseOwnedTable for PlatformTables {
    fn table(name: &'static str) -> OwnedTable {
        debug_assert!(
            Self::OWNED_TABLES.contains(&name),
            "storage reached for a table it does not own: {name}"
        );
        OwnedTable(name)
    }
}

/// Every `(owner, tables)` pair, for the partition test.
///
/// A function rather than a constant because the test needs to read it from
/// outside the crate, and a `pub const` of trait-associated values is not
/// something the type system will let you build without naming each impl.
pub fn table_owners() -> Vec<(&'static str, &'static [&'static str])> {
    vec![
        ("knowledge", KnowledgeTables::OWNED_TABLES),
        ("memory", MemoryTables::OWNED_TABLES),
        ("embedding", EmbeddingTables::OWNED_TABLES),
        ("storage", PlatformTables::OWNED_TABLES),
    ]
}
