//! harness-side SQL template ledger

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use memory_mcp::eval_support::{QueryObserver, SqlObservation};

use super::artifact::sha256_hex;
use super::domain::SqlTemplateRecord;
use super::reducer::latency_stats;

/// Closed classification vocabulary for observed SQL (spec §QueryClass).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryClass {
    EpisodeWrite,
    FactWrite,
    EntityUpsert,
    EdgeProbe,
    RelateEdge,
    AnnSearch,
    AliasLookup,
    EdgeNeighbors,
    ActiveEdgeScan,
    CommunityScan,
    AccessLogWrite,
    ResolveLookup,
    ProvenanceRead,
    Migration,
    Other,
}

impl QueryClass {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::EpisodeWrite => "EpisodeWrite",
            Self::FactWrite => "FactWrite",
            Self::EntityUpsert => "EntityUpsert",
            Self::EdgeProbe => "EdgeProbe",
            Self::RelateEdge => "RelateEdge",
            Self::AnnSearch => "AnnSearch",
            Self::AliasLookup => "AliasLookup",
            Self::EdgeNeighbors => "EdgeNeighbors",
            Self::ActiveEdgeScan => "ActiveEdgeScan",
            Self::CommunityScan => "CommunityScan",
            Self::AccessLogWrite => "AccessLogWrite",
            Self::ResolveLookup => "ResolveLookup",
            Self::ProvenanceRead => "ProvenanceRead",
            Self::Migration => "Migration",
            Self::Other => "Other",
        }
    }
}

/// Distinct templates recorded per run; beyond this, everything merges into
/// one `(overflow)` accumulator so the artifact key-space stays bounded.
pub const MAX_TEMPLATES: usize = 64;

const OVERFLOW_KEY: &str = "(overflow)";

/// First-match-wins rules over `"{op} {lowercased sql}"`.
const RULES: &[(QueryClass, &[&str])] = &[
    (QueryClass::Migration, &["define ", "remove "]),
    (QueryClass::AnnSearch, &["vector::similarity::cosine"]),
    (QueryClass::CommunityScan, &["member_entities containsany"]),
    (QueryClass::AliasLookup, &["containsany"]),
    (QueryClass::RelateEdge, &["relate "]),
    (
        QueryClass::EpisodeWrite,
        &[
            "create episode",
            "update episode",
            "upsert episode",
            "delete episode",
        ],
    ),
    (
        QueryClass::FactWrite,
        &["create fact", "update fact", "upsert fact", "delete fact"],
    ),
    (
        QueryClass::EntityUpsert,
        &[
            "create entity",
            "update entity",
            "upsert entity",
            "delete entity",
        ],
    ),
    (
        QueryClass::AccessLogWrite,
        &[
            "create context_access_log",
            "update context_access_log",
            "create access_log",
            "insert access_log",
        ],
    ),
    (
        QueryClass::EdgeNeighbors,
        &["where in = ", "in, out, relation from edge"],
    ),
    (
        QueryClass::EdgeProbe,
        &["where id = edge:", "where id = $edge", "where edge_id"],
    ),
    (
        QueryClass::ActiveEdgeScan,
        &["from edge where", "from edge order by"],
    ),
    (
        QueryClass::ResolveLookup,
        &["canonical_name", "resolve_lookup"],
    ),
    (
        QueryClass::ProvenanceRead,
        &[
            "from fact",
            "from episode",
            "from claim",
            "from entity",
            "from procedure",
            "from source",
        ],
    ),
];

struct TemplateAcc {
    op: String,
    class: QueryClass,
    latencies: Vec<u64>,
}

#[derive(Default)]
struct LedgerInner {
    templates: BTreeMap<String, TemplateAcc>,
    overflow: Option<TemplateAcc>,
}

/// Mutex-protected template ledger; installed as the eval query observer and
/// read out as one artifact snapshot per scenario.
pub struct QueryLedger {
    inner: Mutex<LedgerInner>,
}

impl QueryLedger {
    pub fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(LedgerInner::default()),
        })
    }

    /// Stable snapshot: one record per distinct template plus `(overflow)`.
    /// Entries with no latency samples (the unset overflow) yield no record.
    pub fn snapshot(&self) -> Vec<SqlTemplateRecord> {
        let inner = self.inner.lock().expect("query ledger mutex poisoned");
        let mut out: Vec<SqlTemplateRecord> = inner
            .templates
            .iter()
            .filter_map(|(hash, acc)| record(hash, acc))
            .collect();
        if let Some(rec) = inner
            .overflow
            .as_ref()
            .and_then(|acc| record(OVERFLOW_KEY, acc))
        {
            out.push(rec);
        }
        out
    }
}

fn record(hash: &str, acc: &TemplateAcc) -> Option<SqlTemplateRecord> {
    let stats = latency_stats(&acc.latencies)?;
    Some(SqlTemplateRecord {
        template_hash: hash.to_owned(),
        op: acc.op.clone(),
        class: acc.class.as_str().to_owned(),
        count: stats.count,
        stats,
    })
}

impl QueryObserver for QueryLedger {
    fn observe(&self, obs: &SqlObservation) {
        let (template, hash) = normalize_sql(&obs.sql);
        let class = classify(obs.op, &template);
        let mut inner = self.inner.lock().expect("query ledger mutex poisoned");
        if let Some(acc) = inner.templates.get_mut(&hash) {
            acc.latencies.push(obs.micros);
            return;
        }
        if inner.templates.len() < MAX_TEMPLATES {
            inner.templates.insert(
                hash,
                TemplateAcc {
                    op: obs.op.to_owned(),
                    class,
                    latencies: vec![obs.micros],
                },
            );
        } else {
            // Overflow keeps the first observation's op/class as its label;
            // later observations of other shapes only merge latency.
            let acc = inner.overflow.get_or_insert_with(|| TemplateAcc {
                op: obs.op.to_owned(),
                class,
                latencies: Vec::new(),
            });
            acc.latencies.push(obs.micros);
        }
    }
}

/// SQL → `(template, template_hash)`: literals → `?`, numbers → `#`,
/// whitespace collapsed. Literals are replaced before numbers so digits
/// inside a literal never leak into the template.
pub fn normalize_sql(sql: &str) -> (String, String) {
    let no_literals = replace_literals(sql);
    let no_numbers = replace_digits(&no_literals);
    let template = no_numbers.split_whitespace().collect::<Vec<_>>().join(" ");
    let hash = sha256_hex(template.as_bytes());
    (template, hash[..16].to_owned())
}

fn replace_literals(sql: &str) -> String {
    let mut out = String::with_capacity(sql.len());
    let mut chars = sql.chars();
    while let Some(c) = chars.next() {
        if c == '\'' {
            out.push('?');
            while let Some(inside) = chars.next() {
                if inside == '\\' {
                    chars.next();
                } else if inside == '\'' {
                    break;
                }
            }
        } else {
            out.push(c);
        }
    }
    out
}

fn replace_digits(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut in_number = false;
    for c in s.chars() {
        if c.is_ascii_digit() {
            if !in_number {
                out.push('#');
                in_number = true;
            }
        } else {
            in_number = false;
            out.push(c);
        }
    }
    out
}

/// Classify by the closed rule table against `op` plus the lowercased
/// template; no rule matched → [`QueryClass::Other`].
pub fn classify(op: &str, sql: &str) -> QueryClass {
    let hay = format!("{} {}", op.to_lowercase(), sql.to_lowercase());
    for (class, markers) in RULES {
        if markers.iter().any(|m| hay.contains(m)) {
            return *class;
        }
    }
    QueryClass::Other
}

#[cfg(test)]
mod tests {
    use super::*;
    use memory_mcp::eval_support::{QueryObserver, SqlObservation};

    fn obs(op: &'static str, sql: &str) -> SqlObservation {
        SqlObservation {
            op,
            sql: sql.to_owned(),
            micros: 100,
        }
    }

    #[test]
    fn normalize_replaces_literals_and_numbers_to_one_template() {
        let (t1, h1) = normalize_sql("SELECT * FROM fact WHERE name = 'Alice' LIMIT 10");
        let (t2, h2) = normalize_sql("SELECT * FROM fact WHERE name = 'Bob' LIMIT 25");
        assert_eq!(t1, "SELECT * FROM fact WHERE name = ? LIMIT #");
        assert_eq!(t1, t2);
        assert_eq!(h1, h2);
    }

    #[test]
    fn digits_and_hashes_inside_literals_do_not_leak_into_the_template() {
        let (t1, h1) = normalize_sql("CREATE episode:1 SET note = 'abc#123'");
        let (t2, h2) = normalize_sql("CREATE episode:9 SET note = 'zz#9'");
        assert_eq!(t1, "CREATE episode:# SET note = ?");
        assert_eq!(t1, t2);
        assert_eq!(h1, h2);
    }

    #[test]
    fn classify_follows_the_rule_table() {
        let cases: &[(&str, &str, QueryClass)] = &[
            (
                "db.execute_query",
                "SELECT *, vector::similarity::cosine(embedding, $query_vec) AS sem_score \
                 FROM fact WHERE embedding IS NOT NONE \
                 AND embedding <|10, 40|> $query_vec ORDER BY sem_score DESC LIMIT $limit",
                QueryClass::AnnSearch,
            ),
            (
                "db.execute_query",
                "SELECT * FROM entity WHERE alias CONTAINSANY $alias",
                QueryClass::AliasLookup,
            ),
            (
                "db.execute_query",
                "SELECT * FROM community WHERE member_entities CONTAINSANY $members ORDER BY community_id ASC",
                QueryClass::CommunityScan,
            ),
            (
                "db.execute_raw_query",
                "LET $a = 1; RELATE $in -> edge:x -> $out SET content = $c RETURN *",
                QueryClass::RelateEdge,
            ),
            (
                "db.execute_raw_query",
                "DEFINE ANALYZER memory_fts TOKENIZERS class FILTERS lowercase;",
                QueryClass::Migration,
            ),
            (
                "db.execute_raw_query",
                "CREATE episode:1 SET content = 'x'",
                QueryClass::EpisodeWrite,
            ),
            (
                "db.execute_query",
                "UPDATE fact:1 SET content = 2",
                QueryClass::FactWrite,
            ),
            (
                "db.execute_query",
                "SELECT * FROM edge WHERE in = edge:a AND relation = 'rel' AND out = edge:b",
                QueryClass::EdgeNeighbors,
            ),
            (
                "db.execute_query",
                "SELECT * FROM edge WHERE id = edge:1",
                QueryClass::EdgeProbe,
            ),
            (
                "db.execute_query",
                "SELECT * FROM edge WHERE t_valid <= time::now() ORDER BY in ASC, out ASC LIMIT #",
                QueryClass::ActiveEdgeScan,
            ),
            (
                "db.execute_query",
                "SELECT id, canonical_name FROM entity WHERE canonical_name = ?",
                QueryClass::ResolveLookup,
            ),
            (
                "db.execute_raw_query",
                "CREATE context_access_log:1 SET principal = 'p'",
                QueryClass::AccessLogWrite,
            ),
            ("db.execute_query", "RETURN 1", QueryClass::Other),
        ];
        for (op, sql, expected) in cases {
            let (template, _) = normalize_sql(sql);
            assert_eq!(
                classify(op, &template),
                *expected,
                "classify mismatch for: {sql}"
            );
        }
    }

    #[test]
    fn beyond_cap_templates_merge_into_one_overflow_record() {
        fn letters(mut i: usize) -> String {
            let mut s = String::new();
            loop {
                s.insert(0, (b'a' + (i % 26) as u8) as char);
                if i < 26 {
                    break;
                }
                i = i / 26 - 1;
            }
            s
        }
        let ledger = QueryLedger::new();
        for i in 0..(MAX_TEMPLATES + 1) {
            ledger.observe(&obs(
                "db.execute_query",
                &format!("SELECT value FROM tbl_{} WHERE x = true", letters(i)),
            ));
        }
        let records = ledger.snapshot();
        assert_eq!(records.len(), MAX_TEMPLATES + 1);
        let overflow = records
            .iter()
            .find(|r| r.template_hash == "(overflow)")
            .expect("overflow record present");
        assert_eq!(overflow.count, 1);
    }

    #[test]
    fn observe_from_two_threads_keeps_both() {
        let ledger = QueryLedger::new();
        std::thread::scope(|s| {
            let a = std::sync::Arc::clone(&ledger);
            let b = std::sync::Arc::clone(&ledger);
            s.spawn(move || a.observe(&obs("db.execute_query", "RETURN 1")));
            s.spawn(move || b.observe(&obs("db.execute_query", "RETURN 1")));
        });
        let records = ledger.snapshot();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].count, 2);
        assert_eq!(records[0].stats.count, 2);
    }
}
