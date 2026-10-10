use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, Utc};
use serde::Deserialize;

use crate::domain::{CorpusSplit, LabelTrust};

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QualityCorpusManifest {
    pub(crate) schema_version: String,
    pub(crate) corpus_id: String,
    pub(crate) revision: String,
    pub(crate) source_bundle_path: String,
    pub(crate) source_sha256: String,
    pub(crate) native_preparation: NativePreparationManifest,
    pub(crate) cases: Vec<QualityCase>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct NativePreparationManifest {
    pub(crate) schema_version: String,
    pub(crate) source_bundle_sha256: String,
    pub(crate) provider_label: String,
    pub(crate) model: Option<String>,
    pub(crate) embedding_signature: String,
    pub(crate) embedding_dimension: usize,
    pub(crate) similarity_threshold: f64,
    pub(crate) index_signature: String,
    pub(crate) extractor_fingerprint: String,
    pub(crate) claim_rollout_stage: String,
    #[serde(default)]
    pub(crate) model_files_sha256: BTreeMap<String, String>,
    #[serde(default)]
    pub(crate) reranking_signature: Option<String>,
}

pub(crate) struct RuntimeRetrievalIdentity<'a> {
    pub(crate) provider_label: &'a str,
    pub(crate) model: Option<&'a str>,
    pub(crate) embedding_signature: &'a str,
    pub(crate) embedding_dimension: usize,
    pub(crate) similarity_threshold: f64,
    pub(crate) index_signature: &'a str,
    pub(crate) extractor_fingerprint: &'a str,
    pub(crate) claim_rollout_stage: &'a str,
    pub(crate) model_files_sha256: &'a BTreeMap<String, String>,
    pub(crate) reranking_signature: Option<&'a str>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QualityCase {
    pub(crate) id: String,
    pub(crate) split: CorpusSplit,
    pub(crate) label_trust: LabelTrust,
    pub(crate) query: String,
    pub(crate) budget: usize,
    #[serde(default)]
    pub(crate) as_of: Option<DateTime<Utc>>,
    /// Transaction cutoff observed after this source's native extraction commit.
    #[serde(default)]
    pub(crate) snapshot_after_source: Option<String>,
    pub(crate) tags: Vec<String>,
    #[serde(default)]
    pub(crate) fact_types: Vec<String>,
    #[serde(default)]
    pub(crate) view_mode: Option<String>,
    #[serde(default)]
    pub(crate) window_start: Option<DateTime<Utc>>,
    #[serde(default)]
    pub(crate) window_end: Option<DateTime<Utc>>,
    #[serde(default)]
    pub(crate) access: Option<memory_mcp::models::AccessPayload>,
    #[serde(default = "default_compact")]
    pub(crate) compact: bool,
    pub(crate) sources: Vec<SourceEpisode>,
    pub(crate) bindings: Vec<QualityFactBinding>,
    #[serde(default)]
    pub(crate) required_evidence_groups: Vec<BTreeSet<String>>,
    #[serde(default)]
    pub(crate) expect_empty: bool,
    #[serde(default)]
    pub(crate) expected_order: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceEpisode {
    pub(crate) key: String,
    pub(crate) source_type: String,
    pub(crate) source_id: String,
    pub(crate) content: String,
    pub(crate) t_ref: DateTime<Utc>,
    #[serde(default)]
    pub(crate) t_ingested: Option<DateTime<Utc>>,
    #[serde(default)]
    pub(crate) policy_tags: Vec<String>,
}

#[derive(Debug, Clone, Deserialize, serde::Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub(crate) struct SourceBundleEntry {
    pub(crate) case_id: String,
    pub(crate) source: SourceEpisode,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct QualityFactBinding {
    pub(crate) key: String,
    pub(crate) episode_key: String,
    pub(crate) fact_type: String,
    /// Exact persisted fact content produced by native extraction.
    pub(crate) content: String,
    pub(crate) disposition: BindingDisposition,
    #[serde(default)]
    pub(crate) kind: EvidenceKind,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Default, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EvidenceKind {
    #[default]
    Fact,
    Episode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum BindingDisposition {
    Allowed,
    Prohibited,
}

impl QualityCorpusManifest {
    pub(crate) fn parse(raw: &str) -> Result<Self, String> {
        let manifest: Self = serde_json::from_str(raw).map_err(|error| error.to_string())?;
        manifest.validate()?;
        Ok(manifest)
    }

    fn validate(&self) -> Result<(), String> {
        if self.schema_version != "memory-mcp-retrieval-quality/v1" {
            return Err(format!(
                "unsupported retrieval quality corpus schema `{}`",
                self.schema_version
            ));
        }
        if self.corpus_id.trim().is_empty() || self.revision.trim().is_empty() {
            return Err("corpus_id and revision must not be empty".into());
        }
        let source_path = std::path::Path::new(&self.source_bundle_path);
        if self.source_bundle_path.trim().is_empty()
            || source_path.is_absolute()
            || source_path
                .components()
                .any(|component| !matches!(component, std::path::Component::Normal(_)))
        {
            return Err("source_bundle_path must be a safe relative file path".into());
        }
        if self.source_sha256.len() != 64
            || !self
                .source_sha256
                .bytes()
                .all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("source_sha256 must be a 64-character hexadecimal digest".into());
        }
        self.native_preparation.validate(&self.source_sha256)?;
        if self.cases.is_empty() {
            return Err("retrieval quality corpus must contain at least one case".into());
        }

        let mut case_ids = BTreeSet::new();
        for case in &self.cases {
            if !case_ids.insert(&case.id) {
                return Err(format!("duplicate case id `{}`", case.id));
            }
            case.validate()?;
        }
        if !self.cases.iter().any(|case| case.expect_empty) {
            return Err("retrieval quality corpus must include an expected-empty case".into());
        }
        if !self
            .cases
            .iter()
            .flat_map(|case| &case.bindings)
            .any(|binding| binding.disposition == BindingDisposition::Prohibited)
        {
            return Err("retrieval quality corpus must include a prohibited-evidence case".into());
        }
        if !self.cases.iter().any(|case| !case.expect_empty) {
            return Err("retrieval quality corpus must include an answerable case".into());
        }
        Ok(())
    }
}

impl NativePreparationManifest {
    fn validate(&self, corpus_source_sha256: &str) -> Result<(), String> {
        if self.schema_version != "memory-mcp-native-preparation/v1" {
            return Err(format!(
                "unsupported native preparation schema `{}`",
                self.schema_version
            ));
        }
        if !self
            .source_bundle_sha256
            .eq_ignore_ascii_case(corpus_source_sha256)
        {
            return Err(
                "native preparation source bundle digest differs from corpus digest".into(),
            );
        }
        if self.provider_label.trim().is_empty()
            || self.embedding_signature.trim().is_empty()
            || self.index_signature.trim().is_empty()
            || self.extractor_fingerprint.trim().is_empty()
            || self.claim_rollout_stage.trim().is_empty()
        {
            return Err("native preparation identity fields must not be empty".into());
        }
        if !is_sha256_signature(&self.embedding_signature, "embsig:")
            || !is_sha256_signature(&self.index_signature, "indexsig:")
        {
            return Err(
                "native preparation provider and index signatures must be SHA-256 identities"
                    .into(),
            );
        }
        if self
            .model
            .as_ref()
            .is_some_and(|model| model.trim().is_empty())
        {
            return Err("native preparation model must be absent or non-empty".into());
        }
        if self.embedding_dimension == 0 {
            return Err("native preparation embedding dimension must be positive".into());
        }
        if self.provider_label == "local-candle"
            && (self.model_files_sha256.len() != 3
                || ["config.json", "tokenizer.json", "model.safetensors"]
                    .iter()
                    .any(|file| {
                        !self.model_files_sha256.get(*file).is_some_and(|digest| {
                            digest.len() == 64
                                && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
                        })
                    }))
        {
            return Err(
                "local native preparation requires config, tokenizer and weights SHA-256".into(),
            );
        }
        if !self.similarity_threshold.is_finite()
            || !(-1.0..=1.0).contains(&self.similarity_threshold)
        {
            return Err(
                "native preparation similarity threshold must be finite and in [-1, 1]".into(),
            );
        }
        Ok(())
    }

    pub(crate) fn validate_runtime(
        &self,
        runtime: &RuntimeRetrievalIdentity<'_>,
    ) -> Result<(), &'static str> {
        let threshold_matches =
            self.similarity_threshold.to_bits() == runtime.similarity_threshold.to_bits();
        if &self.model_files_sha256 != runtime.model_files_sha256
            || self.reranking_signature.as_deref() != runtime.reranking_signature
            || self.provider_label != runtime.provider_label
            || self.model.as_deref() != runtime.model
            || self.embedding_signature != runtime.embedding_signature
            || self.embedding_dimension != runtime.embedding_dimension
            || !threshold_matches
            || self.index_signature != runtime.index_signature
            || self.extractor_fingerprint != runtime.extractor_fingerprint
            || self.claim_rollout_stage != runtime.claim_rollout_stage
        {
            return Err(
                "configured retrieval runtime does not match the frozen native preparation manifest",
            );
        }
        Ok(())
    }
}

fn is_sha256_signature(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|digest| {
        digest.len() == 64 && digest.bytes().all(|byte| byte.is_ascii_hexdigit())
    })
}

impl QualityCase {
    fn validate(&self) -> Result<(), String> {
        if self.id.trim().is_empty() || self.query.trim().is_empty() {
            return Err("case id and query must not be empty".into());
        }
        if self.tags.is_empty()
            || self.tags.iter().any(|tag| tag.trim().is_empty())
            || self.tags.iter().any(|tag| {
                !tag.bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
            })
            || self.tags.iter().collect::<BTreeSet<_>>().len() != self.tags.len()
        {
            return Err(format!(
                "case `{}` must have unique, non-empty, ASCII-safe evaluation tags",
                self.id
            ));
        }
        if self.budget == 0 || self.budget > i32::MAX as usize {
            return Err(format!(
                "case `{}` budget is outside the supported range",
                self.id
            ));
        }
        if self
            .fact_types
            .iter()
            .any(|fact_type| fact_type.trim().is_empty())
        {
            return Err(format!("case `{}` contains an empty fact type", self.id));
        }
        if self
            .window_start
            .zip(self.window_end)
            .is_some_and(|(start, end)| start > end)
        {
            return Err(format!("case `{}` has an inverted time window", self.id));
        }

        let mut episode_keys = BTreeSet::new();
        let mut source_ids = BTreeSet::new();
        if self.sources.is_empty() {
            return Err(format!(
                "case `{}` must contain native source episodes",
                self.id
            ));
        }
        for source in &self.sources {
            if source.key.trim().is_empty()
                || source.source_type.trim().is_empty()
                || source.source_id.trim().is_empty()
                || source.content.trim().is_empty()
            {
                return Err(format!(
                    "case `{}` contains an incomplete source episode",
                    self.id
                ));
            }
            if !episode_keys.insert(&source.key) {
                return Err(format!(
                    "case `{}` has duplicate source key `{}`",
                    self.id, source.key
                ));
            }
            if !source_ids.insert(&source.source_id) {
                return Err(format!(
                    "case `{}` has duplicate source id `{}`",
                    self.id, source.source_id
                ));
            }
        }

        let mut binding_keys = BTreeSet::new();
        if let Some(source_key) = &self.snapshot_after_source {
            if self.as_of.is_some() {
                return Err("snapshot_after_source and as_of are mutually exclusive".into());
            }
            if !episode_keys.contains(source_key) {
                return Err("snapshot_after_source references an unknown source".into());
            }
        }
        let mut selectors = BTreeSet::new();
        for binding in &self.bindings {
            if binding.key.trim().is_empty()
                || binding.fact_type.trim().is_empty()
                || binding.content.trim().is_empty()
            {
                return Err(format!(
                    "case `{}` contains an incomplete gold binding",
                    self.id
                ));
            }
            if !episode_keys.contains(&binding.episode_key) {
                return Err(format!(
                    "case `{}` binding `{}` references unknown source `{}`",
                    self.id, binding.key, binding.episode_key
                ));
            }
            if !binding_keys.insert(&binding.key) {
                return Err(format!(
                    "case `{}` has duplicate binding key `{}`",
                    self.id, binding.key
                ));
            }
            if !selectors.insert((
                binding.kind,
                &binding.episode_key,
                &binding.fact_type,
                &binding.content,
            )) {
                return Err(format!(
                    "case `{}` has duplicate fact selector for binding `{}`",
                    self.id, binding.key
                ));
            }
        }

        self.expectation().validate()
    }

    pub(crate) fn expectation(&self) -> QualityExpectation {
        let allowed_keys = self
            .bindings
            .iter()
            .filter(|binding| binding.disposition == BindingDisposition::Allowed)
            .map(|binding| binding.key.clone())
            .collect();
        let prohibited_keys = self
            .bindings
            .iter()
            .filter(|binding| binding.disposition == BindingDisposition::Prohibited)
            .map(|binding| binding.key.clone())
            .collect();

        QualityExpectation {
            required_evidence_groups: self.required_evidence_groups.clone(),
            allowed_keys,
            prohibited_keys,
            expect_empty: self.expect_empty,
            expected_order: self.expected_order.clone(),
        }
    }
}

fn default_compact() -> bool {
    true
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QualityExpectation {
    /// Each set contains alternative IDs that independently satisfy one required evidence group.
    pub(crate) required_evidence_groups: Vec<BTreeSet<String>>,
    pub(crate) allowed_keys: BTreeSet<String>,
    pub(crate) prohibited_keys: BTreeSet<String>,
    pub(crate) expect_empty: bool,
    pub(crate) expected_order: Option<Vec<String>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct QualityObservation {
    pub(crate) ranked_keys: Vec<String>,
    pub(crate) budget: usize,
}

impl QualityExpectation {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if self.required_evidence_groups.is_empty() && !self.expect_empty {
            return Err("answerable expectation must require evidence".into());
        }
        if self.expect_empty && !self.required_evidence_groups.is_empty() {
            return Err("empty expectation cannot require evidence".into());
        }

        if self.expect_empty && !self.allowed_keys.is_empty() {
            return Err("empty expectation cannot allow returned evidence".into());
        }

        if self.allowed_keys.iter().any(|key| key.trim().is_empty())
            || self.prohibited_keys.iter().any(|key| key.trim().is_empty())
        {
            return Err("gold keys must not be empty".into());
        }

        if self
            .allowed_keys
            .iter()
            .chain(&self.prohibited_keys)
            .any(|key| key.starts_with("__unknown:"))
        {
            return Err("gold keys must not use the reserved __unknown: prefix".into());
        }

        if !self.allowed_keys.is_disjoint(&self.prohibited_keys) {
            return Err("a gold key cannot be both allowed and prohibited".into());
        }

        for group in &self.required_evidence_groups {
            if group.is_empty() {
                return Err("required evidence groups must not be empty".into());
            }
            if !group.is_subset(&self.allowed_keys) {
                return Err("required evidence keys must also be allowed".into());
            }
        }

        if let Some(order) = &self.expected_order {
            let unique: BTreeSet<_> = order.iter().collect();
            if unique.len() != order.len() {
                return Err("expected order must not contain duplicate keys".into());
            }
            if order.iter().any(|key| !self.allowed_keys.contains(key)) {
                return Err("expected order keys must also be allowed".into());
            }
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{QualityCase, QualityCorpusManifest, RuntimeRetrievalIdentity};
    use std::collections::BTreeMap;

    #[test]
    fn native_manifest_can_label_source_episode_fallback_evidence() {
        let result = serde_json::from_value::<super::QualityFactBinding>(serde_json::json!({
            "key":"raw-source", "episode_key":"source", "fact_type":"episode",
            "content":"Mara moved to Edinburgh to join a robotics lab.",
            "kind":"episode", "disposition":"allowed"
        }));
        assert!(
            result.is_ok(),
            "production episode fallback needs source-grounded labels: {result:?}"
        );
    }

    #[test]
    fn native_preparation_records_local_model_file_digests() {
        let result = serde_json::from_value::<super::NativePreparationManifest>(
            serde_json::json!({
                "schema_version":"memory-mcp-native-preparation/v1",
                "source_bundle_sha256":"a".repeat(64), "provider_label":"local-candle",
                "model":"intfloat/multilingual-e5-small", "embedding_signature":format!("embsig:{}","a".repeat(64)),
                "embedding_dimension":384, "similarity_threshold":0.7,
                "index_signature":format!("indexsig:{}","b".repeat(64)),
                "extractor_fingerprint":"anno", "claim_rollout_stage":"evidence",
                "model_files_sha256":{"config.json":"c".repeat(64),"tokenizer.json":"d".repeat(64),"model.safetensors":"e".repeat(64)}
            }),
        );
        assert!(
            result.is_ok(),
            "local native preparation must record actual model bytes: {result:?}"
        );
    }

    #[test]
    fn native_case_accepts_a_snapshot_after_a_durable_source_operation() {
        let result = serde_json::from_value::<QualityCase>(serde_json::json!({
            "id":"history", "split":"test", "label_trust":"reviewed",
            "query":"What was approved before the update?", "budget":5,
            "snapshot_after_source":"approval", "tags":["history"],
            "sources":[], "bindings":[], "expect_empty":true
        }));
        assert!(
            result.is_ok(),
            "durable source snapshots need an explicit query contract: {result:?}"
        );
    }

    #[test]
    fn current_native_case_does_not_require_an_absolute_transaction_cutoff() {
        let value = serde_json::json!({
            "id":"current-snapshot", "split":"test", "label_trust":"reviewed",
            "query":"What changed in the release plan?", "budget":5,
            "tags":["current"], "sources":[], "bindings":[],
            "expect_empty":true
        });

        let result = serde_json::from_value::<QualityCase>(value);

        assert!(
            result.is_ok(),
            "current native queries use the production current-time contract: {result:?}"
        );
    }

    #[test]
    fn native_manifest_records_case_tags_and_preparation_identity() {
        let raw = serde_json::json!({
            "schema_version":"memory-mcp-retrieval-quality/v1",
            "corpus_id":"test-corpus",
            "revision":"r1",
            "source_bundle_path":"source.jsonl",
            "source_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "native_preparation": {
                "schema_version":"memory-mcp-native-preparation/v1",
                "source_bundle_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "provider_label":"openai-compatible",
                "model":"embed-model-v1",
                "embedding_signature":format!("embsig:{}", "a".repeat(64)),
                "embedding_dimension":1536,
                "similarity_threshold":0.7,
                "index_signature":format!("indexsig:{}", "b".repeat(64)),
                "extractor_fingerprint":"anno",
                "claim_rollout_stage":"disabled"
            },
            "cases":[{
                "id":"case-1",
                "split":"test",
                "label_trust":"reviewed",
                "query":"What is the user's preference?",
                "budget":5,
                "as_of":"2026-01-01T00:00:00Z",
                "tags":["preference","paraphrase"],
                "sources":[{
                    "key":"source-1",
                    "source_type":"conversation",
                    "source_id":"source-1",
                    "content":"The user prefers tea.",
                    "t_ref":"2026-01-01T00:00:00Z"
                }],
                "bindings":[{
                    "key":"tea-preference",
                    "episode_key":"source-1",
                    "fact_type":"preference",
                    "content":"The user prefers tea.",
                    "disposition":"allowed"
                },{
                    "key":"tea-prohibited",
                    "episode_key":"source-1",
                    "fact_type":"preference",
                    "content":"A different person prefers coffee.",
                    "disposition":"prohibited"
                }],
                "required_evidence_groups":[["tea-preference"]]
            },{
                "id":"unknown-case",
                "split":"test",
                "label_trust":"reviewed",
                "query":"What is the unknown preference?",
                "budget":5,
                "as_of":"2026-01-01T00:00:00Z",
                "tags":["unknown"],
                "expect_empty":true,
                "sources":[{
                    "key":"source-2",
                    "source_type":"conversation",
                    "source_id":"source-2",
                    "content":"No preference has been shared.",
                    "t_ref":"2026-01-01T00:00:00Z"
                }],
                "bindings":[]
            }]
        }).to_string();

        let manifest = QualityCorpusManifest::parse(&raw)
            .expect("case tags and native preparation identity are part of the corpus contract");

        assert_eq!(manifest.cases[0].tags, ["preference", "paraphrase"]);
        assert_eq!(manifest.native_preparation.embedding_dimension, 1536);
        assert_eq!(manifest.native_preparation.extractor_fingerprint, "anno");
        assert!(
            manifest
                .native_preparation
                .validate_runtime(&RuntimeRetrievalIdentity {
                    provider_label: "openai-compatible",
                    model: Some("embed-model-v1"),
                    embedding_signature: &format!("embsig:{}", "a".repeat(64)),
                    embedding_dimension: 1536,
                    similarity_threshold: 0.7,
                    index_signature: &format!("indexsig:{}", "b".repeat(64)),
                    extractor_fingerprint: "anno",
                    claim_rollout_stage: "disabled",
                    model_files_sha256: &BTreeMap::new(),
                    reranking_signature: None,
                },)
                .is_ok()
        );
        assert!(
            manifest
                .native_preparation
                .validate_runtime(&RuntimeRetrievalIdentity {
                    provider_label: "ollama",
                    model: Some("embed-model-v1"),
                    embedding_signature: &format!("embsig:{}", "a".repeat(64)),
                    embedding_dimension: 1536,
                    similarity_threshold: 0.7,
                    index_signature: &format!("indexsig:{}", "b".repeat(64)),
                    extractor_fingerprint: "anno",
                    claim_rollout_stage: "disabled",
                    model_files_sha256: &BTreeMap::new(),
                    reranking_signature: None,
                },)
                .is_err()
        );
    }

    #[test]
    fn native_manifest_rejects_duplicate_case_ids() {
        let case = |id: &str| {
            serde_json::json!({
                "id": id,
                "split": "test",
                "label_trust": "reviewed",
                "query": "What is the user's preference?",
                "budget": 5,
                "as_of": "2026-01-01T00:00:00Z",
                "tags": ["preference"],
                "sources": [{
                    "key": "source-1",
                    "source_type": "conversation",
                    "source_id": "source-1",
                    "content": "The user prefers tea.",
                    "t_ref": "2026-01-01T00:00:00Z"
                }],
                "bindings": [{
                    "key": "tea-preference",
                    "episode_key": "source-1",
                    "fact_type": "preference",
                    "content": "The user prefers tea.",
                    "disposition": "allowed"
                }],
                "required_evidence_groups": [["tea-preference"]]
            })
        };
        let raw = serde_json::json!({
            "schema_version":"memory-mcp-retrieval-quality/v1",
            "corpus_id":"test-corpus",
            "revision":"r1",
            "source_bundle_path":"source.jsonl",
            "source_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            "native_preparation": {
                "schema_version":"memory-mcp-native-preparation/v1",
                "source_bundle_sha256":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
                "provider_label":"openai-compatible",
                "model":"embed-model-v1",
                "embedding_signature":format!("embsig:{}", "a".repeat(64)),
                "embedding_dimension":1536,
                "similarity_threshold":0.7,
                "index_signature":format!("indexsig:{}", "b".repeat(64)),
                "extractor_fingerprint":"anno",
                "claim_rollout_stage":"disabled"
            },
            "cases":[case("same"),case("same")]
        }).to_string();

        let result = QualityCorpusManifest::parse(&raw);

        assert!(
            result.is_err_and(|error| error.contains("duplicate case id")),
            "duplicate case ids must fail corpus validation"
        );
    }
    #[test]
    fn reranked_runtime_cannot_pass_an_unreranked_native_manifest() {
        let manifest = super::NativePreparationManifest {
            schema_version: "memory-mcp-native-preparation/v1".into(),
            source_bundle_sha256: "a".repeat(64),
            provider_label: "openai-compatible".into(),
            model: Some("embed-model-v1".into()),
            embedding_signature: format!("embsig:{}", "a".repeat(64)),
            embedding_dimension: 1536,
            similarity_threshold: 0.7,
            index_signature: format!("indexsig:{}", "b".repeat(64)),
            extractor_fingerprint: "anno".into(),
            claim_rollout_stage: "disabled".into(),
            model_files_sha256: BTreeMap::new(),
            reranking_signature: None,
        };
        let runtime = RuntimeRetrievalIdentity {
            provider_label: "openai-compatible",
            model: Some("embed-model-v1"),
            embedding_signature: &manifest.embedding_signature,
            embedding_dimension: 1536,
            similarity_threshold: 0.7,
            index_signature: &manifest.index_signature,
            extractor_fingerprint: "anno",
            claim_rollout_stage: "disabled",
            model_files_sha256: &manifest.model_files_sha256,
            reranking_signature: Some(
                "reranksig:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
            ),
        };
        assert!(
            manifest.validate_runtime(&runtime).is_err(),
            "reranker changes must invalidate native comparability"
        );
    }
}
#[test]
fn native_preparation_records_independent_reranker_model_identity() {
    let result = serde_json::from_value::<super::NativePreparationManifest>(serde_json::json!({
        "schema_version":"memory-mcp-native-preparation/v1",
        "source_bundle_sha256":"a".repeat(64), "provider_label":"local-candle",
        "model":"intfloat/multilingual-e5-small", "embedding_signature":format!("embsig:{}","a".repeat(64)),
        "embedding_dimension":384, "similarity_threshold":0.7,
        "index_signature":format!("indexsig:{}","b".repeat(64)),
        "extractor_fingerprint":"anno", "claim_rollout_stage":"evidence",
        "reranking_signature":format!("reranksig:{}", "f".repeat(64)),
        "model_files_sha256":{"config.json":"c".repeat(64),"tokenizer.json":"d".repeat(64),"model.safetensors":"e".repeat(64)}
    }));
    assert!(
        result.is_ok(),
        "local native preparation must record actual model bytes: {result:?}"
    );
}
