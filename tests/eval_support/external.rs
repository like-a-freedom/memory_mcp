use std::collections::BTreeMap;

use chrono::{DateTime, Duration, NaiveDate, NaiveDateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

const DEFAULT_SCOPE: &str = "org";
const DEFAULT_BUDGET: i32 = 10;
const DEFAULT_MIN_RECALL_AT_K: f64 = 1.0;
const FULL_OFFICIAL_DATASET: &str = "full_official_dataset";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
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
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FixtureProvenance {
    pub fixture_kind: &'static str,
    pub source_url: &'static str,
    pub auxiliary_source_url: Option<&'static str>,
    pub source_locator: &'static str,
    pub note: &'static str,
}

pub fn fixture_provenance(kind: DatasetKind) -> FixtureProvenance {
    match kind {
        DatasetKind::LongMemEvalCleaned => FixtureProvenance {
            fixture_kind: FULL_OFFICIAL_DATASET,
            source_url: "https://huggingface.co/datasets/xiaowu0162/longmemeval-cleaned/resolve/main/longmemeval_s_cleaned.json",
            auxiliary_source_url: None,
            source_locator: "500 evaluation instances",
            note: "Full upstream artifact downloaded by scripts/convert_external_evals.py; sampling controlled via MEMORY_MCP_EVAL_SAMPLE_PCT.",
        },
        DatasetKind::LoCoMo => FixtureProvenance {
            fixture_kind: FULL_OFFICIAL_DATASET,
            source_url: "https://raw.githubusercontent.com/snap-research/locomo/main/data/locomo10.json",
            auxiliary_source_url: None,
            source_locator: "10 conversations / 1986 QA items",
            note: "Full upstream benchmark downloaded by scripts/convert_external_evals.py; sampling controlled via MEMORY_MCP_EVAL_SAMPLE_PCT.",
        },
        DatasetKind::PersonaMem => FixtureProvenance {
            fixture_kind: FULL_OFFICIAL_DATASET,
            source_url: "https://huggingface.co/datasets/bowen-upenn/PersonaMem/resolve/main/questions_32k.csv",
            auxiliary_source_url: Some(
                "https://huggingface.co/datasets/bowen-upenn/PersonaMem/resolve/main/shared_contexts_32k.jsonl",
            ),
            source_locator: "589 questions / 37 shared contexts",
            note: "Full upstream sources bundled by scripts/convert_external_evals.py; sampling controlled via MEMORY_MCP_EVAL_SAMPLE_PCT.",
        },
        DatasetKind::PrefEval => FixtureProvenance {
            fixture_kind: FULL_OFFICIAL_DATASET,
            source_url: "https://raw.githubusercontent.com/amazon-science/PrefEval/main/benchmark_dataset/rag_retrieval/simcse_implicit_persona/travel_hotel_overall300_topk_history_persona.json",
            auxiliary_source_url: None,
            source_locator: "52 records; track=travel_hotel_overall300_topk_history_persona",
            note: "Full upstream retrieval track downloaded by scripts/convert_external_evals.py; sampling controlled via MEMORY_MCP_EVAL_SAMPLE_PCT.",
        },
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedExternalRetrievalCase {
    pub id: String,
    pub dataset: String,
    pub description: String,
    pub query: String,
    pub scope: String,
    pub budget: i32,
    pub facts: Vec<NormalizedSeedFact>,
    pub expected: NormalizedRetrievalExpectation,
    #[serde(default)]
    pub metadata: Value,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedSeedFact {
    pub content: String,
    pub t_valid: String,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct NormalizedRetrievalExpectation {
    pub tier: String,
    pub must_contain: Vec<String>,
    #[serde(default = "default_min_recall_at_k")]
    pub min_recall_at_k: f64,
}

pub fn normalize_external_dataset(
    kind: DatasetKind,
    raw: &str,
) -> Result<Vec<NormalizedExternalRetrievalCase>, String> {
    adapter_for(kind).normalize(raw)
}

#[allow(dead_code)]
pub async fn verify_fixture_provenance_against_source(
    kind: DatasetKind,
    local_raw: &str,
) -> Result<(), String> {
    match kind {
        DatasetKind::LongMemEvalCleaned => {
            verify_longmemeval_fixture_against_source(local_raw).await
        }
        DatasetKind::LoCoMo => verify_locomo_fixture_against_source(local_raw).await,
        DatasetKind::PersonaMem => verify_personamem_fixture_against_source(local_raw).await,
        DatasetKind::PrefEval => verify_prefeval_fixture_against_source(local_raw).await,
    }
}

trait ExternalDatasetAdapter {
    fn normalize(&self, raw: &str) -> Result<Vec<NormalizedExternalRetrievalCase>, String>;
}

fn adapter_for(kind: DatasetKind) -> &'static dyn ExternalDatasetAdapter {
    match kind {
        DatasetKind::LongMemEvalCleaned => &LONGMEMEVAL_CLEANED_ADAPTER,
        DatasetKind::LoCoMo => &LOCOMO_ADAPTER,
        DatasetKind::PersonaMem => &PERSONAMEM_ADAPTER,
        DatasetKind::PrefEval => &PREFEVAL_ADAPTER,
    }
}

struct LongMemEvalCleanedAdapter;
struct LoCoMoAdapter;
struct PersonaMemAdapter;
struct PrefEvalAdapter;

static LONGMEMEVAL_CLEANED_ADAPTER: LongMemEvalCleanedAdapter = LongMemEvalCleanedAdapter;
static LOCOMO_ADAPTER: LoCoMoAdapter = LoCoMoAdapter;
static PERSONAMEM_ADAPTER: PersonaMemAdapter = PersonaMemAdapter;
static PREFEVAL_ADAPTER: PrefEvalAdapter = PrefEvalAdapter;

#[derive(Debug, Deserialize)]
struct LongMemEvalRecord {
    question_id: String,
    question_type: String,
    question: String,
    answer: Value,
    question_date: String,
    haystack_dates: Vec<String>,
    haystack_sessions: Vec<Vec<LongMemEvalMessage>>,
}

#[derive(Debug, Deserialize)]
struct LongMemEvalMessage {
    content: String,
}

#[derive(Debug, Deserialize)]
struct LoCoMoRecord {
    #[serde(default)]
    sample_id: Option<String>,
    conversation: LoCoMoConversation,
    qa: Vec<LoCoMoQa>,
    #[serde(default)]
    session_summary: BTreeMap<String, Value>,
    #[serde(default)]
    event_summary: BTreeMap<String, Value>,
    #[serde(default)]
    observation: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
struct LoCoMoConversation {
    speaker_a: String,
    speaker_b: String,
    #[serde(flatten)]
    session_fields: BTreeMap<String, Value>,
}

#[derive(Debug, Deserialize)]
struct LoCoMoMessage {
    #[serde(rename = "dia_id")]
    dia_id: String,
    speaker: String,
    text: String,
}

#[derive(Debug, Clone)]
struct CollectedLoCoMoMessage {
    dia_id: String,
    speaker: String,
    text: String,
    t_valid: String,
}

#[derive(Debug, Deserialize)]
struct LoCoMoQa {
    question: String,
    #[serde(default)]
    answer: Option<Value>,
    #[serde(default)]
    adversarial_answer: Option<String>,
    #[serde(default)]
    evidence: Vec<String>,
    category: i32,
}

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
    topic: String,
    context_length_in_tokens: i64,
    context_length_in_letters: i64,
    distance_to_ref_in_blocks: i64,
    distance_to_ref_in_tokens: i64,
    num_irrelevant_tokens: i64,
    distance_to_ref_proportion_in_context: String,
    user_question_or_message: String,
    correct_answer: String,
    all_options: String,
    shared_context_id: String,
    end_index_in_shared_context: i64,
}

#[derive(Debug, Clone, Deserialize)]
struct PersonaMemContextMessage {
    #[serde(rename = "role")]
    _role: String,
    content: String,
}

#[derive(Debug, Deserialize)]
struct PrefEvalFixture {
    track: String,
    records: Vec<PrefEvalRecord>,
}

#[derive(Debug, Deserialize)]
struct PrefEvalRecord {
    preference: String,
    question: String,
    explanation: String,
    model: String,
    #[serde(default)]
    violation_probability: Option<f64>,
    persona: String,
    conversation: BTreeMap<String, PrefEvalConversationTurn>,
}

#[derive(Debug, Deserialize)]
struct PrefEvalConversationTurn {
    user: String,
    #[serde(rename = "assistant")]
    _assistant: String,
}

impl ExternalDatasetAdapter for LongMemEvalCleanedAdapter {
    fn normalize(&self, raw: &str) -> Result<Vec<NormalizedExternalRetrievalCase>, String> {
        let records: Vec<LongMemEvalRecord> = serde_json::from_str(raw)
            .map_err(|err| format!("parse longmemeval-cleaned dataset: {err}"))?;

        records
            .into_iter()
            .map(normalize_longmemeval_record)
            .collect()
    }
}

impl ExternalDatasetAdapter for LoCoMoAdapter {
    fn normalize(&self, raw: &str) -> Result<Vec<NormalizedExternalRetrievalCase>, String> {
        let records: Vec<LoCoMoRecord> =
            serde_json::from_str(raw).map_err(|err| format!("parse locomo dataset: {err}"))?;

        let mut cases = Vec::new();
        for (record_idx, record) in records.into_iter().enumerate() {
            cases.extend(normalize_locomo_record(record_idx, record)?);
        }
        Ok(cases)
    }
}

impl ExternalDatasetAdapter for PersonaMemAdapter {
    fn normalize(&self, raw: &str) -> Result<Vec<NormalizedExternalRetrievalCase>, String> {
        let PersonaMemFixture {
            questions,
            shared_contexts,
        } = serde_json::from_str(raw).map_err(|err| format!("parse personamem dataset: {err}"))?;

        questions
            .into_iter()
            .map(|question| normalize_personamem_question(question, &shared_contexts))
            .collect()
    }
}

impl ExternalDatasetAdapter for PrefEvalAdapter {
    fn normalize(&self, raw: &str) -> Result<Vec<NormalizedExternalRetrievalCase>, String> {
        let PrefEvalFixture { track, records } =
            serde_json::from_str(raw).map_err(|err| format!("parse prefeval dataset: {err}"))?;

        records
            .into_iter()
            .enumerate()
            .map(|(record_idx, record)| normalize_prefeval_record(&track, record_idx, record))
            .collect()
    }
}

fn normalize_longmemeval_record(
    record: LongMemEvalRecord,
) -> Result<NormalizedExternalRetrievalCase, String> {
    if record.haystack_dates.len() != record.haystack_sessions.len() {
        return Err(format!(
            "record {} has {} session dates but {} sessions",
            record.question_id,
            record.haystack_dates.len(),
            record.haystack_sessions.len()
        ));
    }

    let dataset = DatasetKind::LongMemEvalCleaned.dataset_name().to_string();
    let facts = normalize_longmemeval_facts(
        &record.question_id,
        &record.haystack_dates,
        &record.haystack_sessions,
    )?;

    Ok(NormalizedExternalRetrievalCase {
        id: format!("{}:{}", dataset, record.question_id),
        dataset,
        description: format!("{} [{}]", record.question, record.question_type),
        query: record.question.clone(),
        scope: DEFAULT_SCOPE.to_string(),
        budget: DEFAULT_BUDGET,
        facts,
        expected: NormalizedRetrievalExpectation {
            tier: map_longmemeval_question_type_to_tier(&record.question_type).to_string(),
            must_contain: vec![json_scalar_to_string(&record.answer)],
            min_recall_at_k: DEFAULT_MIN_RECALL_AT_K,
        },
        metadata: json!({
            "question_id": record.question_id,
            "question_type": record.question_type,
            "question_date": parse_longmemeval_datetime(&record.question_date)?,
            "answer": record.answer,
        }),
    })
}

fn normalize_longmemeval_facts(
    question_id: &str,
    dates: &[String],
    sessions: &[Vec<LongMemEvalMessage>],
) -> Result<Vec<NormalizedSeedFact>, String> {
    let mut facts = Vec::new();

    for (session_idx, session) in sessions.iter().enumerate() {
        let t_valid = parse_longmemeval_datetime(&dates[session_idx]).map_err(|err| {
            format!(
                "record {} session {} has invalid date {}: {err}",
                question_id, session_idx, dates[session_idx]
            )
        })?;

        for message in session {
            let content = message.content.trim();
            if content.is_empty() {
                continue;
            }

            facts.push(NormalizedSeedFact {
                content: content.to_string(),
                t_valid: t_valid.clone(),
            });
        }
    }

    Ok(facts)
}

fn normalize_locomo_record(
    record_idx: usize,
    record: LoCoMoRecord,
) -> Result<Vec<NormalizedExternalRetrievalCase>, String> {
    let LoCoMoRecord {
        sample_id,
        conversation,
        qa,
        session_summary,
        event_summary,
        observation,
    } = record;
    let sample_id = sample_id.unwrap_or_else(|| format!("conv-{}", record_idx + 1));
    let dataset = DatasetKind::LoCoMo.dataset_name().to_string();
    let collected_messages = collect_locomo_messages(&sample_id, &conversation)?;
    let mut facts = collected_messages
        .iter()
        .map(|message| NormalizedSeedFact {
            content: format!("{}: {}", message.speaker, message.text),
            t_valid: message.t_valid.clone(),
        })
        .collect::<Vec<_>>();
    let derived_facts = collect_locomo_derived_facts(
        &sample_id,
        &conversation,
        &session_summary,
        &event_summary,
        &observation,
    )?;
    facts.extend(derived_facts.clone());
    dedupe_normalized_facts(&mut facts);
    let speaker_a = conversation.speaker_a.clone();
    let speaker_b = conversation.speaker_b.clone();

    let cases = qa
        .into_iter()
        .enumerate()
        .map(|(qa_idx, qa)| NormalizedExternalRetrievalCase {
            id: format!("{}:{}:{}", dataset, sample_id, qa_idx),
            dataset: dataset.clone(),
            description: format!("{} [category={}]", qa.question, qa.category),
            query: qa.question.clone(),
            scope: DEFAULT_SCOPE.to_string(),
            budget: DEFAULT_BUDGET,
            facts: facts.clone(),
            expected: NormalizedRetrievalExpectation {
                tier: map_locomo_question_to_tier(&qa.question, qa.evidence.len()).to_string(),
                must_contain: locomo_expected_snippets(&collected_messages, &derived_facts, &qa),
                min_recall_at_k: DEFAULT_MIN_RECALL_AT_K,
            },
            metadata: json!({
                "sample_id": sample_id,
                "category": qa.category,
                "evidence": qa.evidence,
                "speaker_a": speaker_a,
                "speaker_b": speaker_b,
            }),
        })
        .collect::<Vec<_>>();

    Ok(cases)
}

fn collect_locomo_messages(
    sample_id: &str,
    conversation: &LoCoMoConversation,
) -> Result<Vec<CollectedLoCoMoMessage>, String> {
    let mut session_indices = conversation
        .session_fields
        .keys()
        .filter_map(|key| {
            key.strip_prefix("session_")
                .and_then(|suffix| suffix.parse::<usize>().ok())
        })
        .collect::<Vec<_>>();
    session_indices.sort_unstable();
    session_indices.dedup();

    let mut messages_out = Vec::new();
    for session_idx in session_indices {
        let session_key = format!("session_{session_idx}");
        let date_key = format!("session_{session_idx}_date_time");

        let Some(messages_value) = conversation.session_fields.get(&session_key) else {
            continue;
        };
        if !messages_value.is_array() {
            continue;
        }
        let Some(date_str) = conversation
            .session_fields
            .get(&date_key)
            .and_then(Value::as_str)
        else {
            return Err(format!(
                "sample {} missing {} for {}",
                sample_id, date_key, session_key
            ));
        };
        let t_valid = parse_locomo_datetime(date_str).map_err(|err| {
            format!(
                "sample {} session {} has invalid date {}: {err}",
                sample_id, session_key, date_str
            )
        })?;
        let messages: Vec<LoCoMoMessage> = serde_json::from_value(messages_value.clone())
            .map_err(|err| format!("sample {sample_id} {session_key} parse error: {err}"))?;

        for message in messages {
            let text = message.text.trim();
            if text.is_empty() {
                continue;
            }

            messages_out.push(CollectedLoCoMoMessage {
                dia_id: message.dia_id,
                speaker: message.speaker,
                text: text.to_string(),
                t_valid: t_valid.clone(),
            });
        }
    }

    Ok(messages_out)
}

fn collect_locomo_derived_facts(
    sample_id: &str,
    conversation: &LoCoMoConversation,
    session_summary: &BTreeMap<String, Value>,
    event_summary: &BTreeMap<String, Value>,
    observation: &BTreeMap<String, Value>,
) -> Result<Vec<NormalizedSeedFact>, String> {
    let session_timestamps = collect_locomo_session_timestamps(sample_id, conversation)?;
    let mut facts = Vec::new();

    for (key, value) in session_summary {
        let Some(session_idx) = parse_locomo_session_key(key, "session_", "_summary") else {
            continue;
        };
        let Some(t_valid) = session_timestamps.get(&session_idx) else {
            continue;
        };
        let Some(summary) = value.as_str() else {
            continue;
        };

        for sentence in sentence_segments(summary) {
            if !sentence.trim().is_empty() {
                facts.push(NormalizedSeedFact {
                    content: sentence,
                    t_valid: t_valid.clone(),
                });
            }
        }
    }

    for (key, value) in event_summary {
        let Some(session_idx) = parse_locomo_session_key(key, "events_session_", "") else {
            continue;
        };
        let Some(t_valid) = session_timestamps.get(&session_idx) else {
            continue;
        };
        let Some(map) = value.as_object() else {
            continue;
        };

        for (speaker, entries) in map {
            if speaker == "date" {
                continue;
            }
            let Some(entries) = entries.as_array() else {
                continue;
            };
            for entry in entries {
                if let Some(text) = locomo_aux_text(entry)
                    && !text.trim().is_empty()
                {
                    facts.push(NormalizedSeedFact {
                        content: text.to_string(),
                        t_valid: t_valid.clone(),
                    });
                }
            }
        }
    }

    for (key, value) in observation {
        let Some(session_idx) = parse_locomo_session_key(key, "session_", "_observation") else {
            continue;
        };
        let Some(t_valid) = session_timestamps.get(&session_idx) else {
            continue;
        };
        let Some(map) = value.as_object() else {
            continue;
        };

        for entries in map.values() {
            let Some(entries) = entries.as_array() else {
                continue;
            };
            for entry in entries {
                if let Some(text) = locomo_aux_text(entry)
                    && !text.trim().is_empty()
                {
                    facts.push(NormalizedSeedFact {
                        content: text.to_string(),
                        t_valid: t_valid.clone(),
                    });
                }
            }
        }
    }

    Ok(facts)
}

fn collect_locomo_session_timestamps(
    sample_id: &str,
    conversation: &LoCoMoConversation,
) -> Result<BTreeMap<usize, String>, String> {
    let mut session_indices = conversation
        .session_fields
        .keys()
        .filter_map(|key| {
            key.strip_prefix("session_")
                .and_then(|suffix| suffix.parse::<usize>().ok())
        })
        .collect::<Vec<_>>();
    session_indices.sort_unstable();
    session_indices.dedup();

    let mut timestamps = BTreeMap::new();
    for session_idx in session_indices {
        let date_key = format!("session_{session_idx}_date_time");
        let Some(date_str) = conversation
            .session_fields
            .get(&date_key)
            .and_then(Value::as_str)
        else {
            continue;
        };
        let t_valid = parse_locomo_datetime(date_str).map_err(|err| {
            format!(
                "sample {} session {} has invalid date {}: {err}",
                sample_id, session_idx, date_str
            )
        })?;
        timestamps.insert(session_idx, t_valid);
    }

    Ok(timestamps)
}

fn parse_locomo_session_key(key: &str, prefix: &str, suffix: &str) -> Option<usize> {
    key.strip_prefix(prefix)
        .and_then(|rest| rest.strip_suffix(suffix).or(Some(rest)))
        .and_then(|digits| digits.parse::<usize>().ok())
}

fn locomo_aux_text(value: &Value) -> Option<&str> {
    value.as_str().or_else(|| {
        value
            .as_array()
            .and_then(|parts| parts.first())
            .and_then(Value::as_str)
    })
}

fn dedupe_normalized_facts(facts: &mut Vec<NormalizedSeedFact>) {
    let mut seen = std::collections::HashSet::new();
    facts.retain(|fact| seen.insert((fact.t_valid.clone(), fact.content.clone())));
}

fn normalize_personamem_question(
    question: PersonaMemQuestion,
    shared_contexts: &BTreeMap<String, Vec<PersonaMemContextMessage>>,
) -> Result<NormalizedExternalRetrievalCase, String> {
    let PersonaMemQuestion {
        persona_id,
        question_id,
        question_type,
        topic,
        context_length_in_tokens,
        context_length_in_letters,
        distance_to_ref_in_blocks,
        distance_to_ref_in_tokens,
        num_irrelevant_tokens,
        distance_to_ref_proportion_in_context,
        user_question_or_message,
        correct_answer,
        all_options,
        shared_context_id,
        end_index_in_shared_context,
    } = question;

    let context_messages = shared_contexts
        .get(&shared_context_id)
        .ok_or_else(|| format!("missing shared context {shared_context_id}"))?;
    let usable_len = usize::try_from(end_index_in_shared_context)
        .ok()
        .map(|limit| limit.min(context_messages.len()))
        .unwrap_or(context_messages.len());
    let usable_context = &context_messages[..usable_len.max(1).min(context_messages.len())];
    let facts = usable_context
        .iter()
        .enumerate()
        .filter_map(|(idx, message)| {
            let content = message.content.trim();
            if content.is_empty() {
                return None;
            }

            Some(NormalizedSeedFact {
                content: content.to_string(),
                t_valid: sequence_timestamp(idx),
            })
        })
        .collect::<Vec<_>>();
    let selected_option = parse_personamem_selected_option(&all_options, &correct_answer)?;
    let expected_snippet =
        derive_personamem_expected_snippet(&user_question_or_message, usable_context)?;
    let dataset = DatasetKind::PersonaMem.dataset_name().to_string();

    Ok(NormalizedExternalRetrievalCase {
        id: format!("{}:{}", dataset, question_id),
        dataset,
        description: format!("{} [{}]", user_question_or_message, question_type),
        query: user_question_or_message,
        scope: DEFAULT_SCOPE.to_string(),
        budget: DEFAULT_BUDGET,
        facts,
        expected: NormalizedRetrievalExpectation {
            tier: map_personamem_question_type_to_tier(&question_type).to_string(),
            must_contain: vec![expected_snippet],
            min_recall_at_k: DEFAULT_MIN_RECALL_AT_K,
        },
        metadata: json!({
            "persona_id": persona_id,
            "question_id": question_id,
            "question_type": question_type,
            "topic": topic,
            "context_length_in_tokens": context_length_in_tokens,
            "context_length_in_letters": context_length_in_letters,
            "distance_to_ref_in_blocks": distance_to_ref_in_blocks,
            "distance_to_ref_in_tokens": distance_to_ref_in_tokens,
            "num_irrelevant_tokens": num_irrelevant_tokens,
            "distance_to_ref_proportion_in_context": distance_to_ref_proportion_in_context,
            "correct_answer": correct_answer,
            "selected_option": selected_option,
            "shared_context_id": shared_context_id,
            "end_index_in_shared_context": end_index_in_shared_context,
        }),
    })
}

fn normalize_prefeval_record(
    track: &str,
    record_idx: usize,
    record: PrefEvalRecord,
) -> Result<NormalizedExternalRetrievalCase, String> {
    let PrefEvalRecord {
        preference,
        question,
        explanation,
        model,
        violation_probability,
        persona,
        conversation,
    } = record;
    let facts = normalize_prefeval_facts(&conversation)?;
    let dataset = DatasetKind::PrefEval.dataset_name().to_string();

    Ok(NormalizedExternalRetrievalCase {
        id: format!("{}:{}:{}", dataset, track, record_idx),
        dataset,
        description: format!("{} [{}]", question, track),
        query: question,
        scope: DEFAULT_SCOPE.to_string(),
        budget: DEFAULT_BUDGET,
        facts,
        expected: NormalizedRetrievalExpectation {
            tier: map_prefeval_track_to_tier(track).to_string(),
            must_contain: vec![preference.clone()],
            min_recall_at_k: DEFAULT_MIN_RECALL_AT_K,
        },
        metadata: json!({
            "track": track,
            "persona": persona,
            "model": model,
            "preference": preference,
            "explanation": explanation,
            "violation_probability": violation_probability,
        }),
    })
}

fn normalize_prefeval_facts(
    conversation: &BTreeMap<String, PrefEvalConversationTurn>,
) -> Result<Vec<NormalizedSeedFact>, String> {
    let mut turns = conversation
        .iter()
        .map(|(turn_key, turn)| {
            let turn_index = turn_key
                .parse::<usize>()
                .map_err(|err| format!("invalid prefeval turn key {turn_key}: {err}"))?;
            Ok((turn_index, turn))
        })
        .collect::<Result<Vec<_>, String>>()?;
    turns.sort_unstable_by_key(|(turn_index, _)| *turn_index);

    let mut facts = Vec::new();
    for (_, turn) in turns {
        for sentence in sentence_segments(turn.user.trim()) {
            facts.push(NormalizedSeedFact {
                t_valid: sequence_timestamp(facts.len()),
                content: format!("User: {sentence}"),
            });
        }
    }

    Ok(facts)
}

fn parse_longmemeval_datetime(raw: &str) -> Result<String, String> {
    let parsed = NaiveDateTime::parse_from_str(raw, "%Y/%m/%d (%a) %H:%M")
        .map_err(|err| format!("invalid longmemeval datetime '{raw}': {err}"))?;
    Ok(DateTime::<Utc>::from_naive_utc_and_offset(parsed, Utc).to_rfc3339())
}

fn parse_locomo_datetime(raw: &str) -> Result<String, String> {
    let parsed = NaiveDateTime::parse_from_str(raw, "%I:%M %p on %d %B, %Y")
        .map_err(|err| format!("invalid locomo datetime '{raw}': {err}"))?;
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

fn map_personamem_question_type_to_tier(question_type: &str) -> &'static str {
    match question_type {
        "recall_user_shared_facts" | "recall_assistant_shared_facts" => "direct",
        other if other.contains("recall") => "direct",
        _ => "reasoning",
    }
}

fn map_prefeval_track_to_tier(track: &str) -> &'static str {
    let normalized = track.to_ascii_lowercase();
    if normalized.contains("implicit") || normalized.ends_with("_persona") {
        "reasoning"
    } else {
        "direct"
    }
}

fn parse_personamem_selected_option(
    all_options: &str,
    correct_answer: &str,
) -> Result<String, String> {
    let options = parse_personamem_option_list(all_options)
        .map_err(|err| format!("parse personamem all_options: {err}"))?;
    let Some(option_char) = correct_answer
        .chars()
        .find(|character| character.is_ascii_alphabetic())
        .map(|character| character.to_ascii_lowercase())
    else {
        return Err(format!(
            "invalid personamem correct_answer label {correct_answer}"
        ));
    };
    let option_index = usize::from((option_char as u8).saturating_sub(b'a'));

    options
        .get(option_index)
        .cloned()
        .ok_or_else(|| format!("missing personamem option {correct_answer}"))
}

fn parse_personamem_option_list(all_options: &str) -> Result<Vec<String>, String> {
    serde_json::from_str(all_options).or_else(|_| parse_personamem_option_literal(all_options))
}

fn parse_personamem_option_literal(all_options: &str) -> Result<Vec<String>, String> {
    let mut characters = all_options.chars().peekable();

    skip_personamem_option_whitespace(&mut characters);
    match characters.next() {
        Some('[') => {}
        other => {
            return Err(format!(
                "expected '[' to start option list, found {:?}",
                other
            ));
        }
    }

    let mut options = Vec::new();

    loop {
        skip_personamem_option_whitespace(&mut characters);
        if matches!(characters.peek(), Some(']')) {
            characters.next();
            break;
        }

        options.push(parse_personamem_option_string(&mut characters)?);

        skip_personamem_option_whitespace(&mut characters);
        match characters.next() {
            Some(',') => continue,
            Some(']') => break,
            other => {
                return Err(format!(
                    "expected ',' or ']' after option, found {:?}",
                    other
                ));
            }
        }
    }

    skip_personamem_option_whitespace(&mut characters);
    if let Some(trailing) = characters.next() {
        return Err(format!(
            "unexpected trailing content after option list starting with {:?}",
            trailing
        ));
    }

    Ok(options)
}

fn parse_personamem_option_string(
    characters: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<String, String> {
    let Some(quote) = characters.next() else {
        return Err("expected quoted personamem option, found end of input".to_string());
    };
    if quote != '\'' && quote != '"' {
        return Err(format!(
            "expected quoted personamem option, found {:?}",
            quote
        ));
    }

    let mut option = String::new();

    while let Some(character) = characters.next() {
        match character {
            '\\' => {
                let Some(escaped) = characters.next() else {
                    return Err("unterminated escape sequence in personamem option".to_string());
                };
                match escaped {
                    '\\' => option.push('\\'),
                    '\'' => option.push('\''),
                    '"' => option.push('"'),
                    'n' => option.push('\n'),
                    'r' => option.push('\r'),
                    't' => option.push('\t'),
                    'u' => {
                        let unicode = parse_personamem_unicode_escape(characters)?;
                        option.push(unicode);
                    }
                    other => option.push(other),
                }
            }
            other if other == quote => return Ok(option),
            other => option.push(other),
        }
    }

    Err("unterminated quoted personamem option".to_string())
}

fn parse_personamem_unicode_escape(
    characters: &mut std::iter::Peekable<std::str::Chars<'_>>,
) -> Result<char, String> {
    let mut hex = String::new();
    for _ in 0..4 {
        let Some(character) = characters.next() else {
            return Err("incomplete unicode escape in personamem option".to_string());
        };
        hex.push(character);
    }

    let value = u32::from_str_radix(&hex, 16)
        .map_err(|err| format!("invalid unicode escape \\u{hex}: {err}"))?;
    char::from_u32(value).ok_or_else(|| format!("invalid unicode scalar \\u{hex}"))
}

fn skip_personamem_option_whitespace(characters: &mut std::iter::Peekable<std::str::Chars<'_>>) {
    while matches!(characters.peek(), Some(character) if character.is_whitespace()) {
        characters.next();
    }
}

fn derive_personamem_expected_snippet(
    query: &str,
    messages: &[PersonaMemContextMessage],
) -> Result<String, String> {
    let query_tokens = normalized_overlap_tokens(query);
    let mut best_match: Option<(usize, usize, String)> = None;

    for message in messages {
        for sentence in sentence_candidates(&message.content) {
            let overlap = overlap_score(&query_tokens, &sentence);
            if overlap == 0 {
                continue;
            }

            let candidate = sentence.trim().to_string();
            let candidate_len = candidate.len();
            let should_replace = match &best_match {
                None => true,
                Some((best_overlap, best_len, _)) => {
                    overlap > *best_overlap
                        || (overlap == *best_overlap && candidate_len > *best_len)
                }
            };

            if should_replace {
                best_match = Some((overlap, candidate_len, candidate));
            }
        }
    }

    best_match
        .map(|(_, _, candidate)| candidate)
        .ok_or_else(|| format!("could not derive personamem snippet for query '{query}'"))
}

fn sentence_candidates(text: &str) -> Vec<String> {
    let stripped = text
        .trim()
        .strip_prefix("User: ")
        .or_else(|| text.trim().strip_prefix("Assistant: "))
        .unwrap_or(text.trim());

    stripped
        .split(['.', '!', '?'])
        .map(str::trim)
        .filter(|sentence| !sentence.is_empty())
        .map(str::to_string)
        .collect()
}

fn sentence_segments(text: &str) -> Vec<String> {
    let mut segments = Vec::new();
    let mut current = String::new();

    for character in text.trim().chars() {
        current.push(character);
        if matches!(character, '.' | '!' | '?') {
            let segment = current.trim();
            if !segment.is_empty() {
                segments.push(segment.to_string());
            }
            current.clear();
        }
    }

    let trailing = current.trim();
    if !trailing.is_empty() {
        segments.push(trailing.to_string());
    }

    segments
}

fn overlap_score(query_tokens: &[String], candidate: &str) -> usize {
    let candidate_tokens = normalized_overlap_tokens(candidate);
    query_tokens
        .iter()
        .filter(|token| candidate_tokens.contains(token))
        .count()
}

fn normalized_overlap_tokens(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .map(|token| token.trim().to_ascii_lowercase())
        .filter(|token| token.len() >= 4)
        .collect()
}

fn locomo_expected_snippets(
    messages: &[CollectedLoCoMoMessage],
    derived_facts: &[NormalizedSeedFact],
    qa: &LoCoMoQa,
) -> Vec<String> {
    let evidence_snippets = qa
        .evidence
        .iter()
        .filter_map(|evidence_id| {
            messages
                .iter()
                .find(|message| message.dia_id == *evidence_id)
                .map(|message| message.text.clone())
        })
        .collect::<Vec<_>>();
    let evidence_candidates = evidence_snippets
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();
    let evidence_context_tokens = evidence_candidates
        .iter()
        .flat_map(|snippet| locomo_query_tokens(snippet))
        .collect::<Vec<_>>();

    let derived_snippets = derived_facts
        .iter()
        .map(|fact| fact.content.as_str())
        .collect::<Vec<_>>();

    let best_evidence = best_locomo_snippet(&evidence_candidates, qa, &[]);
    let best_derived = best_locomo_snippet(&derived_snippets, qa, &evidence_context_tokens);

    if let (Some((evidence_score, _evidence_snippet)), Some((derived_score, derived_snippet))) =
        (&best_evidence, &best_derived)
        && derived_score > evidence_score
    {
        return vec![derived_snippet.clone()];
    }

    if evidence_snippets.is_empty() {
        best_derived
            .map(|(_, snippet)| vec![snippet])
            .unwrap_or_else(|| vec![locomo_answer_text(qa)])
    } else {
        best_evidence
            .map(|(_, snippet)| vec![snippet])
            .unwrap_or(evidence_snippets)
    }
}

fn best_locomo_snippet(
    candidates: &[&str],
    qa: &LoCoMoQa,
    context_tokens: &[String],
) -> Option<(usize, String)> {
    let answer_text = locomo_answer_text(qa);
    let query_tokens = locomo_query_tokens(&qa.question);
    let answer_tokens = locomo_query_tokens(&answer_text);
    let normalized_answer = answer_text.trim().to_ascii_lowercase();

    candidates
        .iter()
        .filter_map(|candidate| {
            let trimmed = candidate.trim();
            if trimmed.is_empty() {
                return None;
            }
            let candidate_tokens = locomo_query_tokens(trimmed);
            if candidate_tokens.is_empty() && normalized_answer.is_empty() {
                return None;
            }

            let query_overlap = query_tokens
                .iter()
                .filter(|token| candidate_tokens.contains(token))
                .count();
            let answer_overlap = answer_tokens
                .iter()
                .filter(|token| candidate_tokens.contains(token))
                .count();
            let context_overlap = context_tokens
                .iter()
                .filter(|token| candidate_tokens.contains(token))
                .count();
            let contains_answer = !normalized_answer.is_empty()
                && trimmed.to_ascii_lowercase().contains(&normalized_answer);
            let score = query_overlap
                + (answer_overlap * 2)
                + context_overlap
                + usize::from(contains_answer) * 2;
            (score > 0).then(|| (score, trimmed.to_string()))
        })
        .max_by(|(left_score, left_snippet), (right_score, right_snippet)| {
            left_score
                .cmp(right_score)
                .then_with(|| right_snippet.len().cmp(&left_snippet.len()))
        })
}

fn locomo_query_tokens(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_alphanumeric())
        .filter_map(normalize_locomo_token)
        .collect()
}

fn normalize_locomo_token(token: &str) -> Option<String> {
    let token = token.trim().to_ascii_lowercase();
    if token.is_empty() || locomo_stopword(&token) {
        return None;
    }

    let normalized = match token.as_str() {
        "went" => "go".to_string(),
        "gave" => "give".to_string(),
        "met" => "meet".to_string(),
        "ran" => "run".to_string(),
        "kids" => "kid".to_string(),
        "agencies" => "agency".to_string(),
        other if other.ends_with("ing") && other.len() > 5 => {
            other.trim_end_matches("ing").to_string()
        }
        other if other.ends_with("ied") && other.len() > 4 => {
            format!("{}y", &other[..other.len() - 3])
        }
        other if other.ends_with("ed") && other.len() > 4 => {
            other.trim_end_matches("ed").to_string()
        }
        other if other.ends_with("ies") && other.len() > 4 => {
            format!("{}y", &other[..other.len() - 3])
        }
        other if other.ends_with('s') && other.len() > 4 => other.trim_end_matches('s').to_string(),
        other => other.to_string(),
    };

    (!normalized.is_empty() && !locomo_stopword(&normalized)).then_some(normalized)
}

fn locomo_stopword(token: &str) -> bool {
    matches!(
        token,
        "a" | "an"
            | "and"
            | "are"
            | "at"
            | "be"
            | "did"
            | "do"
            | "does"
            | "for"
            | "from"
            | "had"
            | "has"
            | "have"
            | "how"
            | "in"
            | "is"
            | "it"
            | "its"
            | "of"
            | "on"
            | "or"
            | "the"
            | "their"
            | "them"
            | "to"
            | "was"
            | "were"
            | "what"
            | "when"
            | "where"
            | "which"
            | "who"
            | "why"
            | "would"
            | "likely"
    )
}

fn locomo_answer_text(qa: &LoCoMoQa) -> String {
    if let Some(answer) = &qa.answer {
        match answer {
            Value::String(text) => text.clone(),
            Value::Number(number) => number.to_string(),
            Value::Bool(boolean) => boolean.to_string(),
            Value::Null => qa
                .adversarial_answer
                .clone()
                .unwrap_or_else(|| "null".to_string()),
            other => other.to_string(),
        }
    } else {
        qa.adversarial_answer
            .clone()
            .unwrap_or_else(|| "answer unavailable".to_string())
    }
}

fn sequence_timestamp(index: usize) -> String {
    let base = NaiveDate::from_ymd_opt(2000, 1, 1)
        .expect("valid fixed date")
        .and_hms_opt(0, 0, 0)
        .expect("valid fixed time");
    DateTime::<Utc>::from_naive_utc_and_offset(base + Duration::minutes(index as i64), Utc)
        .to_rfc3339()
}

fn default_min_recall_at_k() -> f64 {
    DEFAULT_MIN_RECALL_AT_K
}

fn json_scalar_to_string(value: &Value) -> String {
    match value {
        Value::String(text) => text.clone(),
        Value::Number(number) => number.to_string(),
        Value::Bool(boolean) => boolean.to_string(),
        Value::Null => "null".to_string(),
        other => other.to_string(),
    }
}

#[allow(dead_code)]
async fn fetch_source_text(url: &str) -> Result<String, String> {
    let response = reqwest::get(url)
        .await
        .map_err(|err| format!("fetch {url}: {err}"))?
        .error_for_status()
        .map_err(|err| format!("fetch {url}: {err}"))?;

    response
        .text()
        .await
        .map_err(|err| format!("read {url}: {err}"))
}

#[allow(dead_code)]
async fn fetch_line_containing(url: &str, needle: &str) -> Result<String, String> {
    let mut response = reqwest::get(url)
        .await
        .map_err(|err| format!("fetch {url}: {err}"))?
        .error_for_status()
        .map_err(|err| format!("fetch {url}: {err}"))?;
    let mut buffer = Vec::new();

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| format!("stream {url}: {err}"))?
    {
        buffer.extend_from_slice(&chunk);

        while let Some(line_end) = buffer.iter().position(|byte| *byte == b'\n') {
            let line_bytes = buffer.drain(..=line_end).collect::<Vec<_>>();
            let line = String::from_utf8(line_bytes)
                .map_err(|err| format!("decode streamed line from {url}: {err}"))?;
            let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
            if trimmed.contains(needle) {
                return Ok(trimmed.to_string());
            }
        }
    }

    if !buffer.is_empty() {
        let line = String::from_utf8(buffer)
            .map_err(|err| format!("decode trailing streamed line from {url}: {err}"))?;
        let trimmed = line.trim_end_matches('\n').trim_end_matches('\r');
        if trimmed.contains(needle) {
            return Ok(trimmed.to_string());
        }
    }

    Err(format!("could not find {needle:?} in {url}"))
}

#[allow(dead_code)]
async fn stream_contains_all(url: &str, needles: &[&str]) -> Result<(), String> {
    let mut response = reqwest::get(url)
        .await
        .map_err(|err| format!("fetch {url}: {err}"))?
        .error_for_status()
        .map_err(|err| format!("fetch {url}: {err}"))?;
    let mut haystack = String::new();
    let mut remaining = needles.to_vec();

    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|err| format!("stream {url}: {err}"))?
    {
        haystack.push_str(&String::from_utf8_lossy(&chunk));
        remaining.retain(|needle| !haystack.contains(needle));
        if remaining.is_empty() {
            return Ok(());
        }
    }

    Err(format!(
        "could not find expected markers in {url}: {:?}",
        remaining
    ))
}

#[allow(dead_code)]
async fn verify_longmemeval_fixture_against_source(local_raw: &str) -> Result<(), String> {
    let local_records: Vec<Value> = serde_json::from_str(local_raw)
        .map_err(|err| format!("parse local longmemeval fixture: {err}"))?;
    let local_record = local_records
        .first()
        .ok_or_else(|| "longmemeval fixture is empty".to_string())?;
    let question_id = local_record
        .get("question_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "longmemeval fixture missing question_id".to_string())?;
    let question = local_record
        .get("question")
        .and_then(Value::as_str)
        .ok_or_else(|| "longmemeval fixture missing question".to_string())?;
    let answer = local_record
        .get("answer")
        .and_then(Value::as_str)
        .ok_or_else(|| "longmemeval fixture missing answer".to_string())?;
    let session_markers = local_record
        .get("haystack_sessions")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_array)
        .flatten()
        .filter_map(|message| message.get("content").and_then(Value::as_str))
        .collect::<Vec<_>>();
    let mut needles = vec![question_id, question, answer];
    needles.extend(session_markers);

    stream_contains_all(
        fixture_provenance(DatasetKind::LongMemEvalCleaned).source_url,
        &needles,
    )
    .await
}

#[allow(dead_code)]
async fn verify_locomo_fixture_against_source(local_raw: &str) -> Result<(), String> {
    let local_records: Vec<Value> = serde_json::from_str(local_raw)
        .map_err(|err| format!("parse local locomo fixture: {err}"))?;
    let local_record = local_records
        .first()
        .ok_or_else(|| "locomo fixture is empty".to_string())?;
    let sample_id = local_record
        .get("sample_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "locomo fixture missing sample_id".to_string())?;

    let upstream_text =
        fetch_source_text(fixture_provenance(DatasetKind::LoCoMo).source_url).await?;
    let upstream_records: Vec<Value> = serde_json::from_str(&upstream_text)
        .map_err(|err| format!("parse upstream locomo dataset: {err}"))?;
    let upstream_record = upstream_records
        .iter()
        .find(|record| record.get("sample_id").and_then(Value::as_str) == Some(sample_id))
        .ok_or_else(|| format!("upstream locomo dataset missing sample_id {sample_id}"))?;

    assert_locomo_conversation_subset(
        local_record
            .get("conversation")
            .ok_or_else(|| "locomo fixture missing conversation".to_string())?,
        upstream_record
            .get("conversation")
            .ok_or_else(|| format!("upstream locomo record {sample_id} missing conversation"))?,
    )?;
    for key in ["session_summary", "event_summary"] {
        assert_json_subset(
            local_record
                .get(key)
                .ok_or_else(|| format!("locomo fixture missing {key}"))?,
            upstream_record
                .get(key)
                .ok_or_else(|| format!("upstream locomo record {sample_id} missing {key}"))?,
            key,
        )?;
    }
    assert_locomo_observation_subset(
        local_record
            .get("observation")
            .ok_or_else(|| "locomo fixture missing observation".to_string())?,
        upstream_record
            .get("observation")
            .ok_or_else(|| format!("upstream locomo record {sample_id} missing observation"))?,
    )?;

    let local_qa = local_record
        .get("qa")
        .and_then(Value::as_array)
        .ok_or_else(|| "locomo fixture missing qa array".to_string())?;
    let upstream_qa = upstream_record
        .get("qa")
        .and_then(Value::as_array)
        .ok_or_else(|| format!("upstream locomo record {sample_id} missing qa array"))?;
    for qa in local_qa {
        if !upstream_qa.iter().any(|candidate| candidate == qa) {
            return Err(format!(
                "upstream locomo record {sample_id} is missing QA entry {qa}"
            ));
        }
    }

    Ok(())
}

fn assert_locomo_conversation_subset(local: &Value, upstream: &Value) -> Result<(), String> {
    let local_map = local
        .as_object()
        .ok_or_else(|| "locomo local conversation is not an object".to_string())?;
    let upstream_map = upstream
        .as_object()
        .ok_or_else(|| "locomo upstream conversation is not an object".to_string())?;

    for (key, local_value) in local_map {
        let Some(upstream_value) = upstream_map.get(key) else {
            return Err(format!("missing key at locomo.conversation.{key}"));
        };

        if key.starts_with("session_") && local_value.is_array() {
            let local_messages = local_value
                .as_array()
                .ok_or_else(|| format!("locomo conversation {key} is not an array"))?;
            let upstream_messages = upstream_value
                .as_array()
                .ok_or_else(|| format!("upstream locomo conversation {key} is not an array"))?;

            for local_message in local_messages {
                if !upstream_messages
                    .iter()
                    .any(|candidate| candidate == local_message)
                {
                    return Err(format!(
                        "upstream locomo {key} is missing message {local_message}"
                    ));
                }
            }
            continue;
        }

        assert_json_subset(
            local_value,
            upstream_value,
            &format!("locomo.conversation.{key}"),
        )?;
    }

    Ok(())
}

fn assert_locomo_observation_subset(local: &Value, upstream: &Value) -> Result<(), String> {
    let local_sessions = local
        .as_object()
        .ok_or_else(|| "locomo local observation is not an object".to_string())?;
    let upstream_sessions = upstream
        .as_object()
        .ok_or_else(|| "locomo upstream observation is not an object".to_string())?;

    for (session_key, local_session) in local_sessions {
        let upstream_session = upstream_sessions
            .get(session_key)
            .ok_or_else(|| format!("missing observation session {session_key}"))?;
        let local_speakers = local_session
            .as_object()
            .ok_or_else(|| format!("local observation {session_key} is not an object"))?;
        let upstream_speakers = upstream_session
            .as_object()
            .ok_or_else(|| format!("upstream observation {session_key} is not an object"))?;

        for (speaker, local_entries) in local_speakers {
            let upstream_entries = upstream_speakers
                .get(speaker)
                .and_then(Value::as_array)
                .ok_or_else(|| format!("missing observation speaker {session_key}.{speaker}"))?;
            let local_entries = local_entries.as_array().ok_or_else(|| {
                format!("local observation {session_key}.{speaker} is not an array")
            })?;

            for local_entry in local_entries {
                let Some(local_text) = local_entry.as_str() else {
                    return Err(format!(
                        "local observation entry {session_key}.{speaker} is not a string: {local_entry}"
                    ));
                };

                let found = upstream_entries.iter().any(|candidate| match candidate {
                    Value::String(text) => text == local_text,
                    Value::Array(parts) => {
                        parts.first().and_then(Value::as_str) == Some(local_text)
                    }
                    _ => false,
                });

                if !found {
                    return Err(format!(
                        "upstream observation {session_key}.{speaker} is missing text {local_text:?}"
                    ));
                }
            }
        }
    }

    Ok(())
}

#[allow(dead_code)]
async fn verify_personamem_fixture_against_source(local_raw: &str) -> Result<(), String> {
    let local_fixture: Value = serde_json::from_str(local_raw)
        .map_err(|err| format!("parse local personamem fixture: {err}"))?;
    let local_question = local_fixture
        .get("questions")
        .and_then(Value::as_array)
        .and_then(|questions| questions.first())
        .ok_or_else(|| "personamem fixture missing first question".to_string())?;
    let question_id = local_question
        .get("question_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "personamem fixture missing question_id".to_string())?;
    let shared_context_id = local_question
        .get("shared_context_id")
        .and_then(Value::as_str)
        .ok_or_else(|| "personamem fixture missing shared_context_id".to_string())?;
    let user_question = local_question
        .get("user_question_or_message")
        .and_then(Value::as_str)
        .ok_or_else(|| "personamem fixture missing user_question_or_message".to_string())?;
    let correct_answer = local_question
        .get("correct_answer")
        .and_then(Value::as_str)
        .ok_or_else(|| "personamem fixture missing correct_answer".to_string())?;

    let provenance = fixture_provenance(DatasetKind::PersonaMem);
    let question_row = fetch_line_containing(provenance.source_url, question_id).await?;
    for needle in [
        question_id,
        shared_context_id,
        user_question,
        correct_answer,
    ] {
        if !question_row.contains(needle) {
            return Err(format!(
                "upstream personamem questions_32k.csv does not contain expected marker {needle:?}"
            ));
        }
    }

    let contexts_url = provenance
        .auxiliary_source_url
        .ok_or_else(|| "personamem provenance missing auxiliary_source_url".to_string())?;
    let matching_line = fetch_line_containing(contexts_url, shared_context_id).await?;
    let upstream_context: Value = serde_json::from_str(&matching_line)
        .map_err(|err| format!("parse upstream personamem context line: {err}"))?;
    let upstream_messages = upstream_context
        .get(shared_context_id)
        .and_then(Value::as_array)
        .ok_or_else(|| format!("upstream personamem context {shared_context_id} missing array"))?;
    let local_messages = local_fixture
        .get("shared_contexts")
        .and_then(|contexts| contexts.get(shared_context_id))
        .and_then(Value::as_array)
        .ok_or_else(|| format!("local personamem fixture missing context {shared_context_id}"))?;

    for local_message in local_messages {
        if !upstream_messages
            .iter()
            .any(|candidate| candidate == local_message)
        {
            return Err(format!(
                "upstream personamem context {shared_context_id} is missing message {local_message}"
            ));
        }
    }

    Ok(())
}

#[allow(dead_code)]
async fn verify_prefeval_fixture_against_source(local_raw: &str) -> Result<(), String> {
    let local_fixture: Value = serde_json::from_str(local_raw)
        .map_err(|err| format!("parse local prefeval fixture: {err}"))?;
    let local_record = local_fixture
        .get("records")
        .and_then(Value::as_array)
        .and_then(|records| records.first())
        .ok_or_else(|| "prefeval fixture missing first record".to_string())?;
    let question = local_record
        .get("question")
        .and_then(Value::as_str)
        .ok_or_else(|| "prefeval fixture missing question".to_string())?;
    let preference = local_record
        .get("preference")
        .and_then(Value::as_str)
        .ok_or_else(|| "prefeval fixture missing preference".to_string())?;
    let persona = local_record
        .get("persona")
        .and_then(Value::as_str)
        .ok_or_else(|| "prefeval fixture missing persona".to_string())?;
    let conversation = local_record
        .get("conversation")
        .and_then(Value::as_object)
        .ok_or_else(|| "prefeval fixture missing conversation".to_string())?;
    let mut encoded_needles = vec![
        serde_json::to_string(question)
            .map_err(|err| format!("encode prefeval question marker: {err}"))?,
        serde_json::to_string(preference)
            .map_err(|err| format!("encode prefeval preference marker: {err}"))?,
        serde_json::to_string(persona)
            .map_err(|err| format!("encode prefeval persona marker: {err}"))?,
    ];
    for turn in conversation.values() {
        if let Some(user) = turn.get("user").and_then(Value::as_str) {
            encoded_needles.push(
                serde_json::to_string(user)
                    .map_err(|err| format!("encode prefeval user marker: {err}"))?,
            );
        }
        if let Some(assistant) = turn.get("assistant").and_then(Value::as_str) {
            encoded_needles.push(
                serde_json::to_string(assistant)
                    .map_err(|err| format!("encode prefeval assistant marker: {err}"))?,
            );
        }
    }
    let needle_refs = encoded_needles
        .iter()
        .map(String::as_str)
        .collect::<Vec<_>>();

    stream_contains_all(
        fixture_provenance(DatasetKind::PrefEval).source_url,
        &needle_refs,
    )
    .await
}

fn assert_json_subset(expected: &Value, actual: &Value, path: &str) -> Result<(), String> {
    match (expected, actual) {
        (Value::Object(expected_map), Value::Object(actual_map)) => {
            for (key, expected_value) in expected_map {
                let next_path = format!("{path}.{key}");
                let Some(actual_value) = actual_map.get(key) else {
                    return Err(format!("missing key at {next_path}"));
                };
                assert_json_subset(expected_value, actual_value, &next_path)?;
            }
            Ok(())
        }
        (Value::Array(expected_items), Value::Array(actual_items)) => {
            if expected_items.len() > actual_items.len() {
                return Err(format!(
                    "array length mismatch at {path}: expected at least {}, got {}",
                    expected_items.len(),
                    actual_items.len()
                ));
            }
            for (idx, expected_item) in expected_items.iter().enumerate() {
                let Some(actual_item) = actual_items.get(idx) else {
                    return Err(format!("missing array item at {path}[{idx}]"));
                };
                assert_json_subset(expected_item, actual_item, &format!("{path}[{idx}]"))?;
            }
            Ok(())
        }
        _ if expected == actual => Ok(()),
        _ => Err(format!(
            "value mismatch at {path}: expected {expected}, got {actual}"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_longmemeval_datetime_converts_to_rfc3339() {
        let parsed = parse_longmemeval_datetime("2026/03/04 (Wed) 09:00").unwrap();

        assert_eq!(parsed, "2026-03-04T09:00:00+00:00");
    }

    #[test]
    fn parse_locomo_datetime_converts_to_rfc3339() {
        let parsed = parse_locomo_datetime("09:00 AM on 07 May, 2023").unwrap();

        assert_eq!(parsed, "2023-05-07T09:00:00+00:00");
    }

    #[test]
    fn normalize_longmemeval_rejects_mismatched_session_counts() {
        let result = normalize_external_dataset(
            DatasetKind::LongMemEvalCleaned,
            r#"[
                {
                    "question_id": "bad-001",
                    "question_type": "knowledge-update",
                    "question": "Where does Maya work now?",
                    "answer": "Orbital Labs",
                    "question_date": "2026/03/04 (Wed) 09:00",
                    "haystack_dates": ["2026/03/01 (Sun) 09:00"],
                    "haystack_sessions": [
                        [{"role": "user", "content": "First"}],
                        [{"role": "user", "content": "Second"}]
                    ]
                }
            ]"#,
        );

        let error = result.expect_err("expected mismatch error");
        assert!(error.contains("has 1 session dates but 2 sessions"));
    }

    #[test]
    fn normalize_longmemeval_accepts_numeric_answers_from_full_dataset() {
        let cases = normalize_external_dataset(
            DatasetKind::LongMemEvalCleaned,
            r#"[
                {
                    "question_id": "lm-full-001",
                    "question_type": "multi-session",
                    "question": "How many items do I need to pick up?",
                    "answer": 3,
                    "question_date": "2023/02/15 (Wed) 23:50",
                    "haystack_dates": ["2023/02/15 (Wed) 01:41"],
                    "haystack_sessions": [[
                        {"content": "You need to pick up three clothing items from the store."}
                    ]]
                }
            ]"#,
        )
        .expect("normalize longmemeval numeric answer");

        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].id, "longmemeval-cleaned:lm-full-001");
        assert_eq!(cases[0].expected.must_contain, vec!["3"]);
    }

    #[test]
    fn question_type_mapping_is_conservative() {
        assert_eq!(
            map_longmemeval_question_type_to_tier("knowledge-update"),
            "temporal"
        );
        assert_eq!(
            map_longmemeval_question_type_to_tier("single-session-preference"),
            "direct"
        );
        assert_eq!(
            map_longmemeval_question_type_to_tier("unknown-type"),
            "reasoning"
        );
    }

    #[test]
    fn locomo_question_mapping_prefers_temporal_then_reasoning() {
        assert_eq!(
            map_locomo_question_to_tier("When did Caroline go to the LGBTQ support group?", 1),
            "temporal"
        );
        assert_eq!(
            map_locomo_question_to_tier("Why did Caroline change her route?", 2),
            "reasoning"
        );
        assert_eq!(
            map_locomo_question_to_tier("Who brought snacks?", 1),
            "direct"
        );
    }

    #[test]
    fn normalize_locomo_rejects_missing_session_date() {
        let result = normalize_external_dataset(
            DatasetKind::LoCoMo,
            r#"[
                {
                    "sample_id": "locomo-bad-001",
                    "conversation": {
                        "speaker_a": "Caroline",
                        "speaker_b": "Mel",
                        "session_1": [
                            {"speaker": "Caroline", "dia_id": "D1:1", "text": "Hello"}
                        ]
                    },
                    "qa": [
                        {
                            "question": "Who said hello?",
                            "answer": "Caroline",
                            "evidence": ["D1:1"],
                            "category": 1
                        }
                    ]
                }
            ]"#,
        );

        let error = result.expect_err("expected missing date error");
        assert!(error.contains("missing session_1_date_time"));
    }

    #[test]
    fn normalize_locomo_accepts_full_records_without_sample_id_and_string_answers() {
        let cases = normalize_external_dataset(
            DatasetKind::LoCoMo,
            r#"[
                {
                    "conversation": {
                        "speaker_a": "Caroline",
                        "speaker_b": "Melanie",
                        "session_1_date_time": "09:00 AM on 07 May, 2023",
                        "session_1": [
                            {"speaker": "Caroline", "dia_id": "D1:3", "text": "I went to a LGBTQ support group yesterday and it was so powerful."}
                        ]
                    },
                    "qa": [
                        {
                            "question": "When did Caroline go to the LGBTQ support group?",
                            "answer": 2023,
                            "evidence": ["D1:3"],
                            "category": 2
                        }
                    ]
                }
            ]"#,
        )
        .expect("normalize locomo full-shape record");

        assert_eq!(cases.len(), 1);
        assert_eq!(cases[0].dataset, "locomo");
        assert_eq!(cases[0].id, "locomo:conv-1:0");
        assert_eq!(cases[0].facts.len(), 1);
        assert_eq!(
            cases[0].expected.must_contain,
            vec!["I went to a LGBTQ support group yesterday and it was so powerful."]
        );
    }

    #[test]
    fn normalize_locomo_includes_observation_and_summary_facts() {
        let cases = normalize_external_dataset(
            DatasetKind::LoCoMo,
            r#"[
                {
                    "sample_id": "locomo-rich-001",
                    "conversation": {
                        "speaker_a": "Caroline",
                        "speaker_b": "Melanie",
                        "session_1_date_time": "09:00 AM on 07 May, 2023",
                        "session_1": [
                            {
                                "speaker": "Caroline",
                                "dia_id": "D1:5",
                                "text": "The transgender stories were so inspiring!"
                            }
                        ]
                    },
                    "qa": [
                        {
                            "question": "What is Caroline's identity?",
                            "answer": "Transgender woman",
                            "evidence": ["D1:5"],
                            "category": 1
                        }
                    ],
                    "session_summary": {
                        "session_1_summary": "Caroline shared a meaningful update with Melanie about her support group experience."
                    },
                    "event_summary": {
                        "events_session_1": {
                            "Caroline": [
                                "Caroline attends an LGBTQ support group for the first time."
                            ],
                            "Melanie": [],
                            "date": "7 May, 2023"
                        }
                    },
                    "observation": {
                        "session_1_observation": {
                            "Caroline": [
                                "Caroline is a transgender woman."
                            ]
                        }
                    }
                }
            ]"#,
        )
        .expect("normalize locomo rich record");

        assert_eq!(cases.len(), 1);
        let case = &cases[0];
        assert!(
            case.facts
                .iter()
                .any(|fact| fact.content == "Caroline is a transgender woman."),
            "expected observation facts to be included"
        );
        assert!(
            case.facts
                .iter()
                .any(|fact| fact.content.contains("support group for the first time")),
            "expected event summary facts to be included"
        );
        assert!(
            case.facts
                .iter()
                .any(|fact| fact.content.contains("meaningful update with Melanie")),
            "expected session summary facts to be included"
        );
    }

    #[test]
    fn parse_personamem_selected_option_maps_letter_to_option_text() {
        let selected =
            parse_personamem_selected_option(r#"["(a) First", "(b) Second", "(c) Third"]"#, "(c)")
                .expect("parse selected option");

        assert_eq!(selected, "(c) Third");
    }

    #[test]
    fn parse_personamem_selected_option_accepts_python_style_list_literals() {
        let selected = parse_personamem_selected_option(
            "['(a) First', '(b) Second', \"(c) It's Third\", '(d) Fourth']",
            "(c)",
        )
        .expect("parse python-style selected option");

        assert_eq!(selected, "(c) It's Third");
    }

    #[test]
    fn parse_personamem_selected_option_accepts_mixed_quote_list_literals() {
        let selected = parse_personamem_selected_option(
            "[\"(a) First\", '(b) Second', \"(c) It's Third\", '(d) Fourth']",
            "(c)",
        )
        .expect("parse mixed-quote selected option");

        assert_eq!(selected, "(c) It's Third");
    }

    #[test]
    fn derive_personamem_expected_snippet_prefers_high_overlap_sentence() {
        let snippet = derive_personamem_expected_snippet(
            "I recently attended an event where there was a unique blend of modern beats with Pacific sounds.",
            &[
                PersonaMemContextMessage {
                    _role: "user".to_string(),
                    content: "User: I was so thrilled to see that fusion in action! The blend of traditional Pacific sounds with modern beats created a captivating experience that resonated deeply with the audience.".to_string(),
                },
                PersonaMemContextMessage {
                    _role: "system".to_string(),
                    content: "Current user persona: Loves software tools and audio layers.".to_string(),
                },
            ],
        )
        .expect("derive snippet");

        assert_eq!(
            snippet,
            "The blend of traditional Pacific sounds with modern beats created a captivating experience that resonated deeply with the audience"
        );
    }

    #[test]
    fn implicit_prefeval_track_maps_to_reasoning() {
        assert_eq!(
            map_prefeval_track_to_tier("travel_hotel_overall300_topk_history_persona"),
            "reasoning"
        );
        assert_eq!(
            map_prefeval_track_to_tier("shop_home_overall300_topk_history"),
            "direct"
        );
    }

    #[test]
    fn locomo_expected_snippets_prefer_evidence_messages() {
        let snippets = locomo_expected_snippets(
            &[CollectedLoCoMoMessage {
                dia_id: "D2:8".to_string(),
                speaker: "Caroline".to_string(),
                text: "Researching adoption agencies — it's been a dream to have a family and give a loving home to kids who need it.".to_string(),
                t_valid: "2023-05-25T13:14:00+00:00".to_string(),
            }],
            &[],
            &LoCoMoQa {
                question: "What did Caroline research?".to_string(),
                answer: Some(Value::String("Adoption agencies".to_string())),
                adversarial_answer: None,
                evidence: vec!["D2:8".to_string()],
                category: 1,
            },
        );

        assert_eq!(
            snippets,
            vec![
                "Researching adoption agencies — it's been a dream to have a family and give a loving home to kids who need it."
            ]
        );
    }

    #[test]
    fn locomo_expected_snippets_prefer_retrieval_ready_derived_fact() {
        let snippets = locomo_expected_snippets(
            &[CollectedLoCoMoMessage {
                dia_id: "D2:8".to_string(),
                speaker: "Caroline".to_string(),
                text: "Researching adoption agencies — it's been a dream to have a family and give a loving home to kids who need it.".to_string(),
                t_valid: "2023-05-25T13:14:00+00:00".to_string(),
            }],
            &[
                NormalizedSeedFact {
                    content: "Caroline is researching adoption agencies with the dream of having a family and providing a loving home to kids in need.".to_string(),
                    t_valid: "2023-05-25T13:14:00+00:00".to_string(),
                },
                NormalizedSeedFact {
                    content: "Caroline is inspired by her supportive friends and mentors to start researching adoption agencies.".to_string(),
                    t_valid: "2023-05-25T13:14:00+00:00".to_string(),
                },
            ],
            &LoCoMoQa {
                question: "What did Caroline research?".to_string(),
                answer: Some(Value::String("Adoption agencies".to_string())),
                adversarial_answer: None,
                evidence: vec!["D2:8".to_string()],
                category: 1,
            },
        );

        assert_eq!(
            snippets,
            vec![
                "Caroline is researching adoption agencies with the dream of having a family and providing a loving home to kids in need."
            ]
        );
    }

    #[test]
    fn sentence_segments_preserve_preference_sentences() {
        let segments = sentence_segments(
            "Those are excellent ideas, thank you! I usually prefer quieter hotels away from the city center, as I absolutely avoid hotels with a bustling nightlife atmosphere. Do you know of any good venues?",
        );

        assert_eq!(
            segments,
            vec![
                "Those are excellent ideas, thank you!",
                "I usually prefer quieter hotels away from the city center, as I absolutely avoid hotels with a bustling nightlife atmosphere.",
                "Do you know of any good venues?",
            ]
        );
    }

    #[test]
    fn fixture_provenance_marks_external_fixtures_as_trimmed_official_excerpts() {
        let kinds = [
            DatasetKind::LongMemEvalCleaned,
            DatasetKind::LoCoMo,
            DatasetKind::PersonaMem,
            DatasetKind::PrefEval,
        ];

        for kind in kinds {
            let provenance = fixture_provenance(kind);
            assert_eq!(provenance.fixture_kind, FULL_OFFICIAL_DATASET);
            assert!(!provenance.source_url.is_empty());
            assert!(!provenance.source_locator.is_empty());
        }
    }

    #[test]
    fn assert_json_subset_accepts_objects_with_extra_upstream_fields() {
        let expected = json!({
            "sample_id": "conv-26",
            "conversation": {
                "speaker_a": "Caroline",
                "session_1": [{"dia_id": "D1:1"}]
            }
        });
        let actual = json!({
            "sample_id": "conv-26",
            "conversation": {
                "speaker_a": "Caroline",
                "speaker_b": "Melanie",
                "session_1": [{"dia_id": "D1:1", "text": "Hello"}],
                "session_2": []
            },
            "extra": true
        });

        assert!(assert_json_subset(&expected, &actual, "$",).is_ok());
    }

    #[test]
    fn locomo_conversation_subset_allows_trimmed_middle_of_session() {
        let local = json!({
            "speaker_a": "Caroline",
            "session_2": [
                {"speaker": "Melanie", "dia_id": "D2:7", "text": "Thanks"},
                {"speaker": "Caroline", "dia_id": "D2:8", "text": "Researching adoption agencies"}
            ]
        });
        let upstream = json!({
            "speaker_a": "Caroline",
            "speaker_b": "Melanie",
            "session_2": [
                {"speaker": "Caroline", "dia_id": "D2:1", "text": "Earlier"},
                {"speaker": "Melanie", "dia_id": "D2:7", "text": "Thanks"},
                {"speaker": "Caroline", "dia_id": "D2:8", "text": "Researching adoption agencies"},
                {"speaker": "Melanie", "dia_id": "D2:9", "text": "Wow"}
            ]
        });

        assert!(assert_locomo_conversation_subset(&local, &upstream).is_ok());
    }

    #[test]
    fn locomo_observation_subset_accepts_upstream_text_and_diaid_pairs() {
        let local = json!({
            "session_1_observation": {
                "Caroline": [
                    "Caroline attended an LGBTQ support group recently and found the transgender stories inspiring."
                ]
            }
        });
        let upstream = json!({
            "session_1_observation": {
                "Caroline": [[
                    "Caroline attended an LGBTQ support group recently and found the transgender stories inspiring.",
                    "D1:3"
                ]]
            }
        });

        assert!(assert_locomo_observation_subset(&local, &upstream).is_ok());
    }
}
