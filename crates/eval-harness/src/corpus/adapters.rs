use std::collections::BTreeMap;

use crate::corpus::manifest::PreparedCorpus;
use crate::error::EvalError;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum DatasetKind {
    LongMemEvalCleaned,
    LoCoMo,
    PersonaMem,
    PrefEval,
}

impl DatasetKind {
    pub fn dataset_name(self) -> &'static str {
        match self {
            Self::LongMemEvalCleaned => "longmemeval-cleaned",
            Self::LoCoMo => "locomo",
            Self::PersonaMem => "personamem",
            Self::PrefEval => "prefeval",
        }
    }

    pub fn parse_name(s: &str) -> Option<Self> {
        match s {
            "longmemeval-cleaned" => Some(Self::LongMemEvalCleaned),
            "locomo" => Some(Self::LoCoMo),
            "personamem" => Some(Self::PersonaMem),
            "prefeval" => Some(Self::PrefEval),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ExternalCase {
    pub id: String,
    pub dataset: String,
    pub description: String,
    pub query: String,
    pub scope: String,
    pub budget: i32,
    pub facts: Vec<SeedFact>,
    pub expected: RetrievalExpectation,
    #[serde(default)]
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SeedFact {
    pub content: String,
    pub t_valid: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RetrievalExpectation {
    pub tier: String,
    pub must_contain: Vec<String>,
    #[serde(default = "default_min_recall")]
    pub min_recall_at_k: f64,
}

fn default_min_recall() -> f64 {
    1.0
}

use serde::{Deserialize, Serialize};

pub trait DatasetAdapter: Send + Sync {
    fn dataset_kind(&self) -> DatasetKind;
    fn adapter_version(&self) -> &str;
    fn normalize(&self, raw: &str) -> Result<Vec<ExternalCase>, EvalError>;
    fn validate_case(&self, case: &ExternalCase) -> Result<(), EvalError> {
        if case.id.is_empty() {
            return Err(EvalError::InvalidInput("case id must not be empty".into()));
        }
        if case.query.is_empty() {
            return Err(EvalError::InvalidInput(format!(
                "case {} has empty query",
                case.id
            )));
        }
        if case.facts.is_empty() {
            return Err(EvalError::InvalidInput(format!(
                "case {} has no facts",
                case.id
            )));
        }
        Ok(())
    }
}

pub fn adapter_for(kind: DatasetKind) -> Box<dyn DatasetAdapter> {
    match kind {
        DatasetKind::LongMemEvalCleaned => Box::new(LongMemEvalAdapter),
        DatasetKind::LoCoMo => Box::new(LoCoMoAdapter),
        DatasetKind::PersonaMem => Box::new(PersonaMemAdapter),
        DatasetKind::PrefEval => Box::new(PrefEvalAdapter),
    }
}

pub fn load_and_normalize(
    kind: DatasetKind,
    prepared: &PreparedCorpus,
) -> Result<Vec<ExternalCase>, EvalError> {
    let raw = std::fs::read_to_string(&prepared.data_path).map_err(|source| EvalError::Io {
        path: prepared.data_path.clone(),
        source,
    })?;
    let adapter = adapter_for(kind);
    let cases = adapter.normalize(&raw)?;
    for case in &cases {
        adapter.validate_case(case)?;
        crate::domain::EvalCaseId::parse(&case.id).map_err(|error| {
            EvalError::InvalidInput(format!(
                "invalid {} case id '{}': {error}",
                kind.dataset_name(),
                case.id
            ))
        })?;
    }
    Ok(cases)
}

// ─── LongMemEval Adapter ─────────────────────────────────────────────────────

struct LongMemEvalAdapter;

#[derive(Debug, Deserialize)]
struct LongMemEvalRecord {
    question_id: String,
    question_type: String,
    question: String,
    answer: serde_json::Value,
    #[allow(dead_code)]
    question_date: String,
    haystack_dates: Vec<String>,
    haystack_sessions: Vec<Vec<LongMemEvalMessage>>,
}

#[derive(Debug, Deserialize)]
struct LongMemEvalMessage {
    content: String,
}

impl DatasetAdapter for LongMemEvalAdapter {
    fn dataset_kind(&self) -> DatasetKind {
        DatasetKind::LongMemEvalCleaned
    }

    fn adapter_version(&self) -> &str {
        "1"
    }

    fn normalize(&self, raw: &str) -> Result<Vec<ExternalCase>, EvalError> {
        let records: Vec<LongMemEvalRecord> =
            serde_json::from_str(raw).map_err(|e| EvalError::InvalidInput(e.to_string()))?;
        records
            .into_iter()
            .map(normalize_longmemeval_record)
            .collect()
    }
}

fn normalize_longmemeval_record(record: LongMemEvalRecord) -> Result<ExternalCase, EvalError> {
    if record.haystack_dates.len() != record.haystack_sessions.len() {
        return Err(EvalError::InvalidInput(format!(
            "record {} has {} session dates but {} sessions",
            record.question_id,
            record.haystack_dates.len(),
            record.haystack_sessions.len()
        )));
    }

    let dataset = "longmemeval-cleaned".to_string();
    let facts = normalize_longmemeval_facts(
        &record.question_id,
        &record.haystack_dates,
        &record.haystack_sessions,
    )?;

    Ok(ExternalCase {
        id: format!("{}:{}", dataset, record.question_id),
        dataset,
        description: format!("{} [{}]", record.question, record.question_type),
        query: record.question.clone(),
        scope: "org".to_string(),
        budget: 5,
        facts,
        expected: RetrievalExpectation {
            tier: map_longmemeval_question_type_to_tier(&record.question_type).to_string(),
            must_contain: vec![json_scalar_to_string(&record.answer)],
            min_recall_at_k: 1.0,
        },
        metadata: serde_json::json!({
            "question_id": record.question_id,
            "question_type": record.question_type,
        }),
    })
}

fn normalize_longmemeval_facts(
    question_id: &str,
    dates: &[String],
    sessions: &[Vec<LongMemEvalMessage>],
) -> Result<Vec<SeedFact>, EvalError> {
    let mut facts = Vec::new();
    for (session_idx, session) in sessions.iter().enumerate() {
        let t_valid = parse_longmemeval_datetime(&dates[session_idx]).map_err(|e| {
            EvalError::InvalidInput(format!(
                "record {question_id} session {session_idx} has invalid date: {e}"
            ))
        })?;
        for message in session {
            let content = message.content.trim().to_string();
            if !content.is_empty() {
                facts.push(SeedFact {
                    content,
                    t_valid: t_valid.clone(),
                });
            }
        }
    }
    Ok(facts)
}

fn parse_longmemeval_datetime(raw: &str) -> Result<String, String> {
    use chrono::{DateTime, NaiveDateTime, Utc};
    let parsed = NaiveDateTime::parse_from_str(raw, "%Y/%m/%d (%a) %H:%M")
        .map_err(|e| format!("invalid longmemeval datetime '{raw}': {e}"))?;
    Ok(DateTime::<Utc>::from_naive_utc_and_offset(parsed, Utc).to_rfc3339())
}

fn map_longmemeval_question_type_to_tier(question_type: &str) -> &'static str {
    match question_type {
        "temporal-reasoning" | "knowledge-update" => "temporal",
        "single-session-preference" | "single-session-user" | "single-session-fact" => "direct",
        "multi-session-preference" | "multi-session-reasoning" => "reasoning",
        _ => "reasoning",
    }
}

// ─── LoCoMo Adapter ──────────────────────────────────────────────────────────

struct LoCoMoAdapter;

#[derive(Debug, Deserialize)]
struct LoCoMoRecord {
    #[serde(default)]
    sample_id: Option<String>,
    conversation: LoCoMoConversation,
    qa: Vec<LoCoMoQa>,
    #[serde(default)]
    #[allow(dead_code)]
    session_summary: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    #[allow(dead_code)]
    event_summary: BTreeMap<String, serde_json::Value>,
    #[serde(default)]
    #[allow(dead_code)]
    observation: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct LoCoMoConversation {
    #[serde(flatten)]
    fields: BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Deserialize)]
struct LoCoMoQa {
    question: String,
    #[serde(default)]
    answer: Option<serde_json::Value>,
    #[serde(default)]
    evidence: Vec<String>,
    category: i32,
}

impl DatasetAdapter for LoCoMoAdapter {
    fn dataset_kind(&self) -> DatasetKind {
        DatasetKind::LoCoMo
    }

    fn adapter_version(&self) -> &str {
        "1"
    }

    fn normalize(&self, raw: &str) -> Result<Vec<ExternalCase>, EvalError> {
        let records: Vec<LoCoMoRecord> =
            serde_json::from_str(raw).map_err(|e| EvalError::InvalidInput(e.to_string()))?;
        let mut cases = Vec::new();
        for (idx, record) in records.into_iter().enumerate() {
            cases.extend(normalize_locomo_record(idx, record)?);
        }
        Ok(cases)
    }
}

fn normalize_locomo_record(
    record_idx: usize,
    record: LoCoMoRecord,
) -> Result<Vec<ExternalCase>, EvalError> {
    let sample_id = record
        .sample_id
        .unwrap_or_else(|| format!("conv-{record_idx}"));
    let dataset = "locomo".to_string();
    let messages = collect_locomo_messages(&sample_id, &record.conversation)?;
    let facts: Vec<SeedFact> = messages
        .iter()
        .map(|m| SeedFact {
            content: format!("{}: {}", m.speaker, m.text),
            t_valid: m.t_valid.clone(),
        })
        .collect();

    let cases = record
        .qa
        .into_iter()
        .enumerate()
        .map(|(qa_idx, qa)| {
            let must_contain = if let Some(answer) = &qa.answer {
                vec![json_scalar_to_string(answer)]
            } else {
                vec![qa.question.clone()]
            };
            let tier = map_locomo_question_to_tier(&qa.question, qa.evidence.len()).to_string();
            ExternalCase {
                id: format!("{dataset}:{sample_id}:{qa_idx}"),
                dataset: dataset.clone(),
                description: format!("{} [category={}]", qa.question, qa.category),
                query: qa.question,
                scope: "org".to_string(),
                budget: 5,
                facts: facts.clone(),
                expected: RetrievalExpectation {
                    tier,
                    must_contain,
                    min_recall_at_k: 1.0,
                },
                metadata: serde_json::json!({
                    "sample_id": sample_id,
                    "category": qa.category,
                }),
            }
        })
        .collect();

    Ok(cases)
}

struct CollectedLoCoMoMessage {
    #[allow(dead_code)]
    dia_id: String,
    speaker: String,
    text: String,
    t_valid: String,
}

fn collect_locomo_messages(
    sample_id: &str,
    conversation: &LoCoMoConversation,
) -> Result<Vec<CollectedLoCoMoMessage>, EvalError> {
    let mut indices = conversation
        .fields
        .keys()
        .filter_map(|k| {
            k.strip_prefix("session_")
                .and_then(|s| s.parse::<usize>().ok())
        })
        .collect::<Vec<_>>();
    indices.sort_unstable();
    indices.dedup();

    let mut out = Vec::new();
    for idx in indices {
        let session_key = format!("session_{idx}");
        let date_key = format!("session_{idx}_date_time");
        let Some(messages_value) = conversation.fields.get(&session_key) else {
            continue;
        };
        if !messages_value.is_array() {
            continue;
        }
        let Some(date_str) = conversation.fields.get(&date_key).and_then(|v| v.as_str()) else {
            return Err(EvalError::InvalidInput(format!(
                "sample {sample_id} missing {date_key}"
            )));
        };
        let t_valid = parse_locomo_datetime(date_str)?;
        let messages: Vec<LoCoMoMsg> =
            serde_json::from_value(messages_value.clone()).map_err(|e| {
                EvalError::InvalidInput(format!(
                    "sample {sample_id} {session_key} parse error: {e}"
                ))
            })?;
        for msg in messages {
            let text = msg.text.trim().to_string();
            if !text.is_empty() {
                out.push(CollectedLoCoMoMessage {
                    dia_id: msg.dia_id,
                    speaker: msg.speaker,
                    text,
                    t_valid: t_valid.clone(),
                });
            }
        }
    }
    Ok(out)
}

#[derive(Debug, Deserialize)]
struct LoCoMoMsg {
    #[serde(rename = "dia_id")]
    dia_id: String,
    speaker: String,
    text: String,
}

fn parse_locomo_datetime(raw: &str) -> Result<String, EvalError> {
    use chrono::{DateTime, NaiveDateTime, Utc};
    let parsed = NaiveDateTime::parse_from_str(raw, "%I:%M %p on %d %B, %Y")
        .map_err(|e| EvalError::InvalidInput(format!("invalid locomo datetime '{raw}': {e}")))?;
    Ok(DateTime::<Utc>::from_naive_utc_and_offset(parsed, Utc).to_rfc3339())
}

fn map_locomo_question_to_tier(question: &str, evidence_count: usize) -> &'static str {
    let normalized = question.to_ascii_lowercase();
    if normalized.starts_with("when ") || normalized.contains(" when ") {
        return "temporal";
    }
    if evidence_count > 1 {
        return "reasoning";
    }
    "direct"
}

// ─── PersonaMem Adapter ──────────────────────────────────────────────────────

struct PersonaMemAdapter;

#[derive(Debug, Deserialize)]
struct PersonaMemFixture {
    questions: Vec<PersonaMemQuestion>,
    shared_contexts: BTreeMap<String, Vec<PersonaMemContextMessage>>,
}

#[derive(Debug, Deserialize)]
struct PersonaMemQuestion {
    persona_id: i64,
    question_id: String,
    question_type: String,
    user_question_or_message: String,
    correct_answer: String,
    shared_context_id: String,
    end_index_in_shared_context: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct PersonaMemContextMessage {
    #[serde(rename = "role")]
    _role: String,
    content: String,
}

impl DatasetAdapter for PersonaMemAdapter {
    fn dataset_kind(&self) -> DatasetKind {
        DatasetKind::PersonaMem
    }

    fn adapter_version(&self) -> &str {
        "1"
    }

    fn normalize(&self, raw: &str) -> Result<Vec<ExternalCase>, EvalError> {
        let fixture: PersonaMemFixture =
            serde_json::from_str(raw).map_err(|e| EvalError::InvalidInput(e.to_string()))?;
        fixture
            .questions
            .into_iter()
            .map(|q| normalize_personamem_question(q, &fixture.shared_contexts))
            .collect()
    }
}

fn normalize_personamem_question(
    question: PersonaMemQuestion,
    shared_contexts: &BTreeMap<String, Vec<PersonaMemContextMessage>>,
) -> Result<ExternalCase, EvalError> {
    let context_messages = shared_contexts
        .get(&question.shared_context_id)
        .ok_or_else(|| {
            EvalError::InvalidInput(format!(
                "missing shared context {}",
                question.shared_context_id
            ))
        })?;
    let usable_len = usize::try_from(question.end_index_in_shared_context)
        .ok()
        .map(|l| l.min(context_messages.len()))
        .unwrap_or(context_messages.len());
    let usable = &context_messages[..usable_len.max(1).min(context_messages.len())];
    let facts: Vec<SeedFact> = usable
        .iter()
        .enumerate()
        .filter_map(|(idx, msg)| {
            let content = msg.content.trim().to_string();
            if content.is_empty() {
                return None;
            }
            Some(SeedFact {
                content,
                t_valid: sequence_timestamp(idx),
            })
        })
        .collect();

    let dataset = "personamem".to_string();
    Ok(ExternalCase {
        id: format!("{}:{}", dataset, question.question_id),
        dataset,
        description: format!(
            "{} [{}]",
            question.user_question_or_message, question.question_type
        ),
        query: question.user_question_or_message,
        scope: "org".to_string(),
        budget: 5,
        facts,
        expected: RetrievalExpectation {
            tier: map_personamem_question_type_to_tier(&question.question_type).to_string(),
            must_contain: vec![question.correct_answer],
            min_recall_at_k: 1.0,
        },
        metadata: serde_json::json!({
            "persona_id": question.persona_id,
            "question_id": question.question_id,
            "question_type": question.question_type,
        }),
    })
}

fn map_personamem_question_type_to_tier(question_type: &str) -> &'static str {
    match question_type {
        t if t.contains("recall") => "direct",
        _ => "reasoning",
    }
}

// ─── PrefEval Adapter ────────────────────────────────────────────────────────

struct PrefEvalAdapter;

#[derive(Debug, Deserialize)]
struct PrefEvalFixture {
    track: String,
    records: Vec<PrefEvalRecord>,
}

#[derive(Debug, Deserialize)]
struct PrefEvalRecord {
    preference: String,
    question: String,
    persona: String,
    conversation: BTreeMap<String, PrefEvalTurn>,
}

#[derive(Debug, Deserialize)]
struct PrefEvalTurn {
    user: String,
}

impl DatasetAdapter for PrefEvalAdapter {
    fn dataset_kind(&self) -> DatasetKind {
        DatasetKind::PrefEval
    }

    fn adapter_version(&self) -> &str {
        "1"
    }

    fn normalize(&self, raw: &str) -> Result<Vec<ExternalCase>, EvalError> {
        let fixture: PrefEvalFixture =
            serde_json::from_str(raw).map_err(|e| EvalError::InvalidInput(e.to_string()))?;
        fixture
            .records
            .into_iter()
            .enumerate()
            .map(|(idx, record)| normalize_prefeval_record(&fixture.track, idx, record))
            .collect()
    }
}

fn normalize_prefeval_record(
    track: &str,
    record_idx: usize,
    record: PrefEvalRecord,
) -> Result<ExternalCase, EvalError> {
    let facts = normalize_prefeval_facts(&record.conversation)?;
    let dataset = "prefeval".to_string();

    Ok(ExternalCase {
        id: format!("{}:{}:{}", dataset, track, record_idx),
        dataset,
        description: format!("{} [{}]", record.question, track),
        query: record.question,
        scope: "org".to_string(),
        budget: 5,
        facts,
        expected: RetrievalExpectation {
            tier: map_prefeval_track_to_tier(track).to_string(),
            must_contain: vec![record.preference],
            min_recall_at_k: 1.0,
        },
        metadata: serde_json::json!({
            "track": track,
            "persona": record.persona,
        }),
    })
}

fn normalize_prefeval_facts(
    conversation: &BTreeMap<String, PrefEvalTurn>,
) -> Result<Vec<SeedFact>, EvalError> {
    let mut turns: Vec<_> = conversation
        .iter()
        .filter_map(|(k, v)| k.parse::<usize>().ok().map(|i| (i, v)))
        .collect();
    turns.sort_by_key(|(i, _)| *i);

    let mut facts = Vec::new();
    for (_, turn) in turns {
        for sentence in sentence_segments(turn.user.trim()) {
            facts.push(SeedFact {
                t_valid: sequence_timestamp(facts.len()),
                content: format!("User: {sentence}"),
            });
        }
    }
    Ok(facts)
}

fn map_prefeval_track_to_tier(track: &str) -> &'static str {
    let n = track.to_ascii_lowercase();
    if n.contains("implicit") || n.ends_with("_persona") {
        "reasoning"
    } else {
        "direct"
    }
}

// ─── Shared Helpers ──────────────────────────────────────────────────────────

fn sequence_timestamp(index: usize) -> String {
    use chrono::{DateTime, Duration, NaiveDate, Utc};
    let base = NaiveDate::from_ymd_opt(2000, 1, 1)
        .expect("valid fixed date")
        .and_hms_opt(0, 0, 0)
        .expect("valid fixed time");
    DateTime::<Utc>::from_naive_utc_and_offset(base + Duration::minutes(index as i64), Utc)
        .to_rfc3339()
}

fn json_scalar_to_string(value: &serde_json::Value) -> String {
    match value {
        serde_json::Value::String(t) => t.clone(),
        serde_json::Value::Number(n) => n.to_string(),
        serde_json::Value::Bool(b) => b.to_string(),
        serde_json::Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

fn sentence_segments(text: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();
    for ch in text.trim().chars() {
        current.push(ch);
        if matches!(ch, '.' | '!' | '?') {
            let seg = current.trim().to_string();
            if !seg.is_empty() {
                segments.push(seg);
            }
            current.clear();
        }
    }
    let trailing = current.trim().to_string();
    if !trailing.is_empty() {
        segments.push(trailing);
    }
    segments
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn longmemeval_adapter_normalizes() {
        let raw = r#"[
            {
                "question_id": "q1",
                "question_type": "single-session-fact",
                "question": "Where does Maya work?",
                "answer": "Orbital Labs",
                "question_date": "2026/03/04 (Wed) 09:00",
                "haystack_dates": ["2026/03/01 (Sun) 09:00"],
                "haystack_sessions": [[{"content": "Maya works at Orbital Labs."}]]
            }
        ]"#;
        let adapter = LongMemEvalAdapter;
        let cases = adapter.normalize(raw).unwrap();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].id, "longmemeval-cleaned:q1");
        assert_eq!(cases[0].expected.tier, "direct");
        assert_eq!(cases[0].expected.must_contain, vec!["Orbital Labs"]);
    }

    #[test]
    fn longmemeval_rejects_mismatched_sessions() {
        let raw = r#"[
            {
                "question_id": "bad",
                "question_type": "knowledge-update",
                "question": "Q?",
                "answer": "A",
                "question_date": "2026/03/04 (Wed) 09:00",
                "haystack_dates": ["2026/03/01 (Sun) 09:00"],
                "haystack_sessions": [[{"content": "First"}], [{"content": "Second"}]]
            }
        ]"#;
        let adapter = LongMemEvalAdapter;
        assert!(adapter.normalize(raw).is_err());
    }

    #[test]
    fn locomo_adapter_normalizes() {
        let raw = r#"[
            {
                "sample_id": "s1",
                "conversation": {
                    "speaker_a": "Alice",
                    "speaker_b": "Bob",
                    "session_0_date_time": "09:00 AM on 07 May, 2023",
                    "session_0": [{"dia_id": "d1", "speaker": "Alice", "text": "Hello Bob"}]
                },
                "qa": [{"question": "Who spoke first?", "answer": "Alice", "evidence": ["d1"], "category": 1}]
            }
        ]"#;
        let adapter = LoCoMoAdapter;
        let cases = adapter.normalize(raw).unwrap();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].id, "locomo:s1:0");
        assert!(!cases[0].facts.is_empty());
    }

    #[test]
    fn personamem_adapter_normalizes() {
        let raw = r#"{
            "questions": [{
                "persona_id": 1,
                "question_id": "pm1",
                "question_type": "recall_user_shared_facts",
                "topic": "pets",
                "context_length_in_tokens": 100,
                "context_length_in_letters": 500,
                "distance_to_ref_in_blocks": 1,
                "distance_to_ref_in_tokens": 50,
                "num_irrelevant_tokens": 10,
                "distance_to_ref_proportion_in_context": "0.5",
                "user_question_or_message": "What pet does she have?",
                "correct_answer": "A cat named Luna",
                "all_options": "[\"A cat named Luna\", \"A dog named Rex\"]",
                "shared_context_id": "ctx1",
                "end_index_in_shared_context": 2
            }],
            "shared_contexts": {
                "ctx1": [
                    {"role": "user", "content": "I have a cat named Luna."},
                    {"role": "assistant", "content": "That's nice!"}
                ]
            }
        }"#;
        let adapter = PersonaMemAdapter;
        let cases = adapter.normalize(raw).unwrap();
        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].expected.tier, "direct");
    }

    #[test]
    fn prefeval_adapter_normalizes() {
        let raw = r#"{
            "track": "test_track",
            "records": [{
                "preference": "I prefer window seats",
                "question": "Do you like aisle or window?",
                "explanation": "prefers window",
                "model": "gpt-4",
                "persona": "traveler",
                "conversation": {
                    "0": {"user": "I always book window seats.", "assistant": "Noted."}
                }
            }]
        }"#;
        let adapter = PrefEvalAdapter;
        let cases = adapter.normalize(raw).unwrap();
        assert_eq!(cases.len(), 1);
        assert_eq!(
            cases[0].expected.must_contain,
            vec!["I prefer window seats"]
        );
    }

    #[test]
    fn every_case_has_stable_id_and_nonempty_stratum() {
        let raw = r#"[
            {
                "question_id": "q1",
                "question_type": "single-session-fact",
                "question": "Q?",
                "answer": "A",
                "question_date": "2026/03/04 (Wed) 09:00",
                "haystack_dates": ["2026/03/01 (Sun) 09:00"],
                "haystack_sessions": [[{"content": "fact"}]]
            }
        ]"#;
        let adapter = LongMemEvalAdapter;
        let cases = adapter.normalize(raw).unwrap();
        for case in &cases {
            assert!(!case.id.is_empty());
            assert!(!case.expected.tier.is_empty());
        }
    }

    #[test]
    fn dataset_kind_from_str_roundtrip() {
        for kind in [
            DatasetKind::LongMemEvalCleaned,
            DatasetKind::LoCoMo,
            DatasetKind::PersonaMem,
            DatasetKind::PrefEval,
        ] {
            let name = kind.dataset_name();
            assert_eq!(DatasetKind::parse_name(name), Some(kind));
        }
    }
}
