//! Optional independent query/passage relevance evidence, applied before budget
//! selection and again to synthesized/episode items before caching the result.

use super::{RetrievalContext, types::RankedContextFact};
use crate::{error::MemoryError, models::AssembledContextItem};
use std::collections::{HashMap, HashSet};

const SOURCE_CONTEXT_RERANK_RADIUS: u64 = 4;
const QUESTION_FUNCTION_TERMS: &[&str] = &[
    "a", "an", "and", "are", "as", "at", "be", "been", "but", "by", "can", "could", "did", "do",
    "does", "for", "from", "had", "has", "have", "how", "i", "in", "is", "it", "of", "on", "or",
    "the", "their", "them", "there", "these", "they", "this", "those", "to", "was", "were", "what",
    "when", "where", "which", "who", "whom", "whose", "why", "will", "with", "would", "you",
    "your",
];

pub(super) fn candidate_pool_limit(budget: usize, reranking_enabled: bool) -> usize {
    let scaled = budget.max(1).saturating_mul(4);
    if reranking_enabled {
        scaled.max(50)
    } else {
        scaled
    }
}

async fn scores(
    ctx: &RetrievalContext,
    query: &str,
    documents: &[String],
) -> Result<Option<Vec<f64>>, MemoryError> {
    if query.trim().is_empty() || documents.is_empty() {
        return Ok(None);
    }
    let provider = &ctx.embedding_provider;
    let scores = ctx.embedding_service.rerank(query, documents).await?;
    if let Some(ref values) = scores {
        if provider.reranking_signature().is_none()
            || !provider.reranking_admission_floor().is_finite()
            || values.len() != documents.len()
            || values.iter().any(|value| !value.is_finite())
        {
            return Err(MemoryError::Validation(
                "invalid reranking evidence identity, cardinality or finite score".into(),
            ));
        }
    } else if provider.reranking_signature().is_some() {
        return Err(MemoryError::Validation(
            "configured reranker returned no evidence".into(),
        ));
    }
    Ok(scores)
}

/// Keep the original query as the primary relevance signal, then add bounded
/// noun/list facets so one cross-encoder comparison cannot hide a second
/// independently requested part of a compound question.
fn query_facets(query: &str) -> Vec<String> {
    const MAX_FACETS: usize = 6;
    let primary = query.trim();
    if primary.is_empty() {
        return Vec::new();
    }

    let mut facets = vec![primary.to_owned()];
    let mut seen = HashSet::from([primary.to_lowercase()]);
    for clause in primary.split([',', ';']) {
        let clause = clause.trim();
        let clause = strip_machine_scope(clause);
        let clause = clause
            .strip_prefix("and ")
            .or_else(|| clause.strip_prefix("or "))
            .unwrap_or(clause)
            .trim();
        if !clause.is_empty() && seen.insert(clause.to_lowercase()) {
            facets.push(clause.to_owned());
        }
        if facets.len() >= MAX_FACETS {
            break;
        }

        if let Some((left, right)) = conjunctive_facets(clause) {
            for facet in [left, right] {
                if facets.len() >= MAX_FACETS {
                    break;
                }
                if seen.insert(facet.to_lowercase()) {
                    facets.push(facet);
                }
            }
        }
        if facets.len() >= MAX_FACETS {
            break;
        }
    }
    facets
}

fn conjunctive_facets(clause: &str) -> Option<(String, String)> {
    // ASCII-only folding keeps byte offsets valid for slices of the original
    // UTF-8 string while still recognizing uppercase ASCII conjunctions.
    let clause_lower = clause.to_ascii_lowercase();
    let connector = clause_lower.find(" and ")?;
    let left = clause[..connector].trim();
    let right = clause[connector + 5..].trim();
    let left_words = left.split_whitespace().collect::<Vec<_>>();
    let right_words = right.split_whitespace().collect::<Vec<_>>();
    let left_last = *left_words.last()?;
    let right_first = *right_words.first()?;

    // Coordinated subjects ("How do Jon and Gina ...") are one question,
    // unlike requested object lists ("Which body measurements and diagnostics").
    let preceding_word = left_words.iter().rev().nth(1).copied().unwrap_or_default();
    if matches!(
        preceding_word.to_ascii_lowercase().as_str(),
        "do" | "does" | "did" | "is" | "are" | "was" | "were" | "has" | "have" | "can" | "will"
    ) || right_words
        .get(1)
        .is_some_and(|word| word.eq_ignore_ascii_case("both"))
        || (starts_uppercase(left_last) && starts_uppercase(right_first))
    {
        return None;
    }

    let left_facet = left_words
        .iter()
        .rev()
        .take(2)
        .copied()
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect::<Vec<_>>()
        .join(" ");
    let right_head = right_words
        .iter()
        .take_while(|word| {
            !matches!(
                word.to_ascii_lowercase().as_str(),
                "do" | "does"
                    | "did"
                    | "is"
                    | "are"
                    | "was"
                    | "were"
                    | "has"
                    | "have"
                    | "can"
                    | "will"
            )
        })
        .take(2)
        .copied()
        .collect::<Vec<_>>()
        .join(" ");
    let scope_terms = right_words
        .iter()
        .skip_while(|word| {
            !matches!(
                word.to_ascii_lowercase().as_str(),
                "do" | "does"
                    | "did"
                    | "is"
                    | "are"
                    | "was"
                    | "were"
                    | "has"
                    | "have"
                    | "can"
                    | "will"
            )
        })
        .skip(1)
        .filter(|word| {
            !matches!(
                word.trim_matches(|character: char| !character.is_alphanumeric())
                    .to_ascii_lowercase()
                    .as_str(),
                "a" | "an"
                    | "the"
                    | "this"
                    | "that"
                    | "these"
                    | "those"
                    | "require"
                    | "requires"
                    | "need"
                    | "needs"
                    | "want"
                    | "wants"
            )
        })
        .take(4)
        .copied()
        .collect::<Vec<_>>();
    let right_facet = std::iter::once(right_head)
        .chain(scope_terms.iter().map(|term| term.to_string()))
        .filter(|term| !term.is_empty())
        .collect::<Vec<_>>()
        .join(" ");
    if left_facet.is_empty() || right_facet.is_empty() {
        return None;
    }
    Some((left_facet, right_facet))
}

fn starts_uppercase(value: &str) -> bool {
    value.chars().next().is_some_and(char::is_uppercase)
}

fn strip_machine_scope(clause: &str) -> &str {
    let Some((facet, scope)) = clause.rsplit_once(" in ") else {
        return clause;
    };
    let scope = scope.trim().trim_matches(|character: char| {
        !character.is_alphanumeric() && !matches!(character, '_' | ':' | '/')
    });
    let is_machine_identifier = scope.contains(['_', ':', '/']);
    if is_machine_identifier && !facet.trim().is_empty() {
        facet.trim()
    } else {
        clause
    }
}

fn matched_secondary_query_facet(
    query: &str,
    content: &str,
    _index_keys: &[String],
) -> Option<String> {
    let facets = query_facets(query);
    if facets.len() < 2 {
        return None;
    }

    let content_terms = crate::shared::search::search_query_terms(content)
        .into_iter()
        .collect::<HashSet<_>>();
    facets
        .into_iter()
        .skip(1)
        .map(|facet| {
            let facet_terms = crate::shared::search::search_query_terms(&facet);
            let required_matches = (facet_terms.len() * 3)
                .div_ceil(5)
                .max(2)
                .min(facet_terms.len());
            let matched_terms = facet_terms
                .into_iter()
                .filter(|term| content_terms.contains(term))
                .collect::<HashSet<_>>();
            (facet, matched_terms.len(), required_matches)
        })
        .filter(|(_, matched_terms, required_matches)| {
            *required_matches > 0 && matched_terms >= required_matches
        })
        .max_by(
            |(left_facet, left_matches, _), (right_facet, right_matches, _)| {
                left_matches
                    .cmp(right_matches)
                    .then_with(|| right_facet.cmp(left_facet))
            },
        )
        .map(|(facet, _, _)| facet)
}

async fn scores_with_query_facets(
    ctx: &RetrievalContext,
    query: &str,
    documents: &[String],
) -> Result<Option<Vec<f64>>, MemoryError> {
    let Some(mut best_scores) = scores(ctx, query, documents).await? else {
        return Ok(None);
    };
    for facet in query_facets(query).into_iter().skip(1) {
        let Some(facet_scores) = scores(ctx, &facet, documents).await? else {
            return Ok(None);
        };
        for (best, facet_score) in best_scores.iter_mut().zip(facet_scores) {
            *best = best.max(facet_score);
        }
    }
    Ok(Some(best_scores))
}

fn evidence(ctx: &RetrievalContext, score: f64) -> serde_json::Value {
    serde_json::json!({"model_signature": ctx.embedding_provider.reranking_signature(),
        "relevance_logit": score,
        "admission_floor": ctx.embedding_provider.reranking_admission_floor()})
}

fn has_independent_admission_evidence(candidate: &RankedContextFact) -> bool {
    candidate.evidence.explicit_date_admitted
        || candidate.evidence.admission_basis == Some(super::types::AdmissionBasis::Reference)
        || candidate.evidence.admission_basis
            == Some(super::types::AdmissionBasis::CalendarAcquisition)
        || candidate.evidence.admission_basis == Some(super::types::AdmissionBasis::Lexical)
            && (!candidate.evidence.matched_content_terms.is_empty()
                || candidate.evidence.native_full_text_match)
        || candidate.retrieval_tier == super::types::RetrievalTier::Direct
            && !candidate.matched_query_terms.is_empty()
}

fn item_has_exact_reference_admission(item: &AssembledContextItem) -> bool {
    item.provenance
        .get("retrieval_admission")
        .and_then(|admission| admission.get("basis"))
        .and_then(serde_json::Value::as_str)
        == Some("reference")
}

fn item_has_calendar_acquisition_admission(item: &AssembledContextItem) -> bool {
    item.provenance
        .get("retrieval_admission")
        .and_then(|admission| admission.get("basis"))
        .and_then(serde_json::Value::as_str)
        == Some("calendar_acquisition")
}

fn item_has_qualified_source_episode_admission(item: &AssembledContextItem) -> bool {
    let Some(admission) = item.provenance.get("retrieval_admission") else {
        return false;
    };
    admission.get("basis").and_then(serde_json::Value::as_str) == Some("qualified_source_episode")
        && admission
            .get("source_context_relation")
            .and_then(serde_json::Value::as_str)
            .is_some_and(|relation| {
                matches!(
                    relation,
                    "adjacent_question_answer"
                        | "anaphoric_follow_up_answer"
                        | "query_related_antecedent"
                        | "query_related_conversation_turn"
                )
            })
}

fn item_has_explicit_date_admission(item: &AssembledContextItem) -> bool {
    item.provenance
        .get("retrieval_admission")
        .and_then(|admission| admission.get("explicit_date_admitted"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
}

fn document(
    content: &str,
    date: Option<chrono::DateTime<chrono::Utc>>,
    source: Option<&str>,
    source_context: Option<&str>,
) -> String {
    let mut context = String::new();
    if let Some(date) = date {
        context.push_str(&format!(
            "Memory reference date: {} ({}).\n",
            date.format("%Y-%m-%d"),
            date.format("%d %B %Y")
        ));
    }
    if let Some(source) = source {
        context.push_str(&format!("Source identity: {source}\n"));
    }
    context.push_str(content);
    if let Some(source_context) = source_context {
        context.push_str("\nSource-ordered dialogue context:\n");
        context.push_str(source_context);
    }
    context
}

pub(super) fn source_question_matches_query(
    content: &str,
    query: &str,
    subject_terms: &[String],
) -> bool {
    if super::admission::source_question_conflicts_with_query_preference(
        query,
        subject_terms,
        content,
    ) || source_question_has_unrequested_follow_up_modifier(content, query)
    {
        return false;
    }
    let query_terms = question_focus_terms(query, subject_terms);
    if query_terms.is_empty() {
        return false;
    }
    let has_answer_slot = question_answer_slot(query, subject_terms).is_some();
    let required_overlap = query_terms.len().min(2);
    let body = source_assertion_text(content)
        .split_once(':')
        .map_or(source_assertion_text(content), |(_, body)| body);
    source_question_clauses(body).into_iter().any(|question| {
        if has_answer_slot {
            return source_question_matches_answer_slot(query, question, subject_terms);
        }
        question_topic_overlap(question, query, subject_terms) >= required_overlap
    })
}

/// A specific "what/which <noun phrase>" query asks for a value of that
/// phrase's head type. Requiring the source question to share the head keeps a
/// nearby question about the same broad topic from supplying a different kind
/// of answer (for example, motivation instead of programming languages).
struct QuestionAnswerSlot {
    head: String,
    modifiers: HashSet<String>,
    relation: Option<String>,
    subject_terms: HashSet<String>,
}

fn question_answer_slot(query: &str, subject_terms: &[String]) -> Option<QuestionAnswerSlot> {
    const PREPOSITION_BOUNDARIES: &[&str] = &[
        "about", "at", "by", "for", "from", "in", "of", "on", "to", "with",
    ];
    const PREDICATE_BOUNDARIES: &[&str] = &[
        "am", "are", "be", "been", "being", "can", "could", "did", "do", "does", "had", "has",
        "have", "is", "may", "might", "must", "shall", "should", "was", "were", "will", "would",
    ];
    const GENERIC_SLOT_MODIFIERS: &[&str] = &["category", "kind", "sort", "type", "variety"];
    const SUBJECT_PRONOUNS: &[&str] = &[
        "he", "her", "hers", "him", "his", "i", "me", "mine", "my", "she", "they", "them", "their",
        "theirs", "we", "us", "our", "ours", "you", "your", "yours",
    ];
    let original_tokens = query
        .split(|character: char| !character.is_alphabetic())
        .filter(|token| !token.is_empty())
        .collect::<Vec<_>>();
    let tokens = original_tokens
        .iter()
        .map(|token| token.to_ascii_lowercase())
        .collect::<Vec<_>>();
    let mut token_start = 1;
    if !matches!(tokens.first()?.as_str(), "what" | "which") {
        return None;
    }
    let copular = tokens
        .get(token_start)
        .is_some_and(|token| matches!(token.as_str(), "is" | "are" | "was" | "were"));
    if copular {
        token_start += 1;
    }
    let mut subject_terms = subject_terms
        .iter()
        .map(|term| canonical_question_term(term))
        .collect::<HashSet<_>>();
    let boundary = (token_start..tokens.len()).find(|index| {
        PREDICATE_BOUNDARIES.contains(&tokens[*index].as_str())
            || copular && PREPOSITION_BOUNDARIES.contains(&tokens[*index].as_str())
    });
    if let Some(boundary) = boundary {
        subject_terms.extend(
            original_tokens[boundary + 1..]
                .iter()
                .filter(|token| token.chars().next().is_some_and(char::is_uppercase))
                .flat_map(|token| crate::shared::search::search_query_terms(token))
                .map(|term| canonical_question_term(&term)),
        );
    }
    let slot_end = boundary.unwrap_or(tokens.len());
    let slot_terms = tokens[token_start..slot_end]
        .iter()
        .flat_map(|token| crate::shared::search::search_query_terms(token))
        .map(|term| canonical_question_term(&term))
        .collect::<Vec<_>>();
    let head = slot_terms.last()?.clone();
    let modifiers = slot_terms
        .iter()
        .take(slot_terms.len().saturating_sub(1))
        .filter(|term| {
            let term = term.as_str();
            !GENERIC_SLOT_MODIFIERS.contains(&term)
                && !SUBJECT_PRONOUNS.contains(&term)
                && !subject_terms.contains(term)
        })
        .cloned()
        .collect::<HashSet<_>>();
    let relation = boundary.and_then(|boundary| {
        tokens[boundary + 1..].iter().find_map(|token| {
            if matches!(
                token.as_str(),
                "have" | "has" | "had" | "own" | "owns" | "owned"
            ) {
                return Some("possess".to_string());
            }
            let term = crate::shared::search::search_query_terms(token)
                .into_iter()
                .next()
                .map(|term| canonical_question_term(&term))?;
            if term == "keep" {
                return Some("possess".to_string());
            }
            if PREDICATE_BOUNDARIES.contains(&token.as_str())
                || QUESTION_FUNCTION_TERMS.contains(&token.as_str())
                || SUBJECT_PRONOUNS.contains(&token.as_str())
                || subject_terms.contains(&term)
            {
                return None;
            }
            Some(term)
        })
    });

    Some(QuestionAnswerSlot {
        head,
        modifiers,
        relation,
        subject_terms,
    })
}

/// Apply explicit type-and-relation checks for answer slots whose values have
/// a bounded, well-known vocabulary. A reranker score is still useful for
/// ordering, but topical similarity alone cannot establish that a passage
/// contains a language or an owned pet.
pub(super) fn typed_answer_slot_support(
    query: &str,
    content: &str,
    subject_terms: &[String],
) -> Option<bool> {
    let slot = question_answer_slot(query, subject_terms)?;
    match slot.head.as_str() {
        "language" if slot.modifiers.contains("programming") => {
            programming_language_slot_supported(&slot, content)
        }
        "pet" => Some(pet_slot_supported(&slot, content)),
        _ => None,
    }
}

fn programming_language_slot_supported(slot: &QuestionAnswerSlot, content: &str) -> Option<bool> {
    let relation = slot.relation.as_deref()?;
    let programming_relations = ["work", "use", "know", "code", "program"];
    let learning_relations = ["learn", "study"];
    let accepted_relations = if learning_relations.contains(&relation) {
        &learning_relations[..]
    } else if programming_relations.contains(&relation) {
        &programming_relations[..]
    } else {
        // The slot parser does not understand every natural-language relation.
        // An unknown relation is not evidence that the candidate is wrong.
        return None;
    };
    const LANGUAGE_VALUES: &[&str] = &[
        "bash",
        "c++",
        "c#",
        "cobol",
        "dart",
        "elixir",
        "erlang",
        "fortran",
        "golang",
        "haskell",
        "java",
        "javascript",
        "kotlin",
        "lua",
        "matlab",
        "objective-c",
        "perl",
        "php",
        "python",
        "ruby",
        "rust",
        "scala",
        "sql",
        "swift",
        "typescript",
        "visual basic",
    ];
    let assertion = source_assertion_text(content).to_ascii_lowercase();
    let mut matching_relation = false;
    let has_supported_language = assertion.split(['.', '!', '?', '\n']).any(|sentence| {
        let words = sentence
            .split(|character: char| !character.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_ascii_lowercase)
            .map(|word| canonical_question_term(&word))
            .collect::<Vec<_>>();
        let states_requested_relation = words
            .iter()
            .any(|word| accepted_relations.contains(&word.as_str()));
        if !states_requested_relation {
            return false;
        }
        matching_relation = true;
        let names_programming_language = LANGUAGE_VALUES.iter().any(|value| {
            if value.contains(['+', '#', '-', ' ']) {
                assertion_contains_phrase(sentence, value)
            } else {
                words.iter().any(|word| word == value)
            }
        });
        let explicitly_limits_programming_languages = {
            let words = sentence
                .split(|character: char| !character.is_alphanumeric())
                .filter(|word| !word.is_empty())
                .map(str::to_ascii_lowercase)
                .collect::<Vec<_>>();
            words
                .windows(2)
                .any(|pair| pair == ["programming", "languages"])
                && words
                    .iter()
                    .any(|word| matches!(word.as_str(), "any" | "other" | "none" | "no" | "only"))
        };
        names_programming_language || explicitly_limits_programming_languages
    });
    if has_supported_language {
        Some(true)
    } else if matching_relation {
        // A supported relation with an unlisted value is unresolved, not a
        // contradiction. Let ordinary retrieval and ranking decide whether it
        // is useful evidence.
        None
    } else {
        Some(false)
    }
}

fn pet_slot_supported(slot: &QuestionAnswerSlot, content: &str) -> bool {
    if slot.relation.as_deref() != Some("possess") {
        return false;
    }
    const PET_TYPES: &[&str] = &[
        "bird", "cat", "chicken", "dog", "ferret", "fish", "frog", "gecko", "gerbil", "guinea",
        "hamster", "hedgehog", "horse", "iguana", "lizard", "mouse", "mice", "parrot", "pig",
        "pony", "rabbit", "rat", "snake", "spider", "tortoise", "turtle",
    ];
    const FIRST_PERSON_POSSESSIVES: &[&str] = &["my", "our"];
    const POSSESSIVE_MODIFIERS: &[&str] = &[
        "first", "one", "second", "third", "fourth", "fifth", "pet", "two", "three", "four",
        "five", "six", "seven", "eight", "nine", "ten",
    ];
    const OWNERSHIP_VERBS: &[&str] = &[
        "adopt", "adopted", "have", "has", "had", "keep", "keeps", "kept", "own", "owns", "owned",
        "rescue", "rescued",
    ];
    const FIRST_PERSON_SUBJECTS: &[&str] = &["i", "we"];

    let assertion = source_assertion_text(content);
    let speaker_matches_subject = assertion.split_once(':').is_some_and(|(speaker, _)| {
        let speaker_terms = speaker
            .split(|character: char| !character.is_alphabetic())
            .filter(|term| !term.is_empty())
            .map(|term| canonical_question_term(&term.to_ascii_lowercase()))
            .collect::<HashSet<_>>();
        slot.subject_terms
            .iter()
            .any(|term| speaker_terms.contains(&canonical_question_term(term)))
    });
    let subject_terms = slot
        .subject_terms
        .iter()
        .map(|term| canonical_question_term(term))
        .collect::<HashSet<_>>();

    assertion.split(['.', '!', '?', '\n']).any(|sentence| {
        let words = sentence
            .split(|character: char| !character.is_alphanumeric())
            .filter(|word| !word.is_empty())
            .map(str::to_ascii_lowercase)
            .collect::<Vec<_>>();
        words.iter().enumerate().any(|(pet_index, word)| {
            let pet_type = canonical_question_term(word);
            if !PET_TYPES.contains(&pet_type.as_str()) {
                return false;
            }
            let ownership_verb = words[..pet_index]
                .iter()
                .rposition(|word| OWNERSHIP_VERBS.contains(&word.as_str()));
            if let Some(verb_index) = ownership_verb {
                let Some(owner_index) = verb_index.checked_sub(1) else {
                    return false;
                };
                let owner = words[owner_index].as_str();
                return subject_terms.contains(&canonical_question_term(owner))
                    || speaker_matches_subject && FIRST_PERSON_SUBJECTS.contains(&owner);
            }

            if !speaker_matches_subject {
                return false;
            }
            let preceding = words[..pet_index]
                .iter()
                .rev()
                .take(3)
                .map(String::as_str)
                .collect::<Vec<_>>();
            preceding.iter().enumerate().any(|(distance, word)| {
                FIRST_PERSON_POSSESSIVES.contains(word)
                    && preceding[..distance]
                        .iter()
                        .all(|modifier| POSSESSIVE_MODIFIERS.contains(modifier))
            })
        })
    })
}

fn assertion_contains_phrase(text: &str, phrase: &str) -> bool {
    text.match_indices(phrase).any(|(start, matched)| {
        let end = start + matched.len();
        let left_is_boundary = start == 0 || !text.as_bytes()[start - 1].is_ascii_alphanumeric();
        let right_is_boundary = end == text.len() || !text.as_bytes()[end].is_ascii_alphanumeric();
        left_is_boundary && right_is_boundary
    })
}

fn source_question_matches_answer_slot(
    query: &str,
    question: &str,
    subject_terms: &[String],
) -> bool {
    let Some(query_slot) = question_answer_slot(query, subject_terms) else {
        return false;
    };
    let Some(source_slot) = question_answer_slot(question, subject_terms) else {
        return false;
    };
    if query_slot.head != source_slot.head
        || !query_slot.modifiers.is_subset(&source_slot.modifiers)
    {
        return false;
    }

    match (&query_slot.relation, &source_slot.relation) {
        (Some(query), Some(source)) => {
            super::admission::source_question_predicates_compatible(query, source)
        }
        (Some(_), None) => false,
        (None, _) => true,
    }
}

fn source_answer_mentions_query_slot(answer: &str, query: &str) -> bool {
    let Some(slot_head) = question_answer_slot(query, &[]).map(|slot| slot.head) else {
        return false;
    };
    let body = source_assertion_text(answer)
        .split_once(':')
        .map_or(source_assertion_text(answer), |(_, body)| body);
    body.split(|character: char| !character.is_alphabetic())
        .filter(|token| !token.is_empty())
        .any(|token| {
            let token = token.to_ascii_lowercase();
            crate::shared::search::search_query_terms(&token)
                .iter()
                .any(|term| canonical_question_term(term) == slot_head)
                || token
                    .strip_prefix(&slot_head)
                    .is_some_and(|suffix| matches!(suffix, "s" | "es" | "ed" | "ing"))
        })
}

pub(super) fn source_question_matches_query_via_context(
    content: &str,
    query: &str,
    subject_terms: &[String],
    anchor: &str,
) -> bool {
    if super::admission::source_question_conflicts_with_query_preference(
        query,
        subject_terms,
        content,
    ) {
        return false;
    }
    if source_question_matches_query(content, query, subject_terms) {
        return true;
    }

    let body = source_assertion_text(content)
        .split_once(':')
        .map_or(source_assertion_text(content), |(_, body)| body);
    let has_answer_slot = question_answer_slot(query, subject_terms).is_some();
    source_question_clauses(body).into_iter().any(|question| {
        let question_is_grounded_in_anchor =
            source_question_is_supported_by_assertion(question, query, anchor, subject_terms);
        if asks_for_calendar_date(query)
            && asks_for_duration(question)
            && source_question_contains_anaphoric_reference(question)
            && has_explicit_duration(anchor)
            && question_is_grounded_in_anchor
        {
            return true;
        }
        if has_answer_slot && !source_question_matches_answer_slot(query, question, subject_terms) {
            return false;
        }
        if !same_specific_interrogative(query, question) {
            return false;
        }
        if question_is_grounded_in_anchor {
            return true;
        }
        is_underspecified_location_follow_up(question)
            && source_context_query_overlap(query, anchor) > 0
    })
}

pub(super) fn source_question_is_context_only_follow_up(
    content: &str,
    query: &str,
    subject_terms: &[String],
) -> bool {
    if super::admission::source_question_conflicts_with_query_preference(
        query,
        subject_terms,
        content,
    ) || !source_question_has_unrequested_follow_up_modifier(content, query)
    {
        return false;
    }
    let query_terms = question_focus_terms(query, subject_terms);
    let required_overlap = query_terms.len().min(2);
    let body = source_assertion_text(content)
        .split_once(':')
        .map_or(source_assertion_text(content), |(_, body)| body);
    let has_answer_slot = question_answer_slot(query, subject_terms).is_some();
    source_question_clauses(body).into_iter().any(|question| {
        (!has_answer_slot || source_question_matches_answer_slot(query, question, subject_terms))
            && question_topic_overlap(question, query, subject_terms) >= required_overlap
    })
}

fn is_underspecified_location_follow_up(question: &str) -> bool {
    let lower = question.to_ascii_lowercase();
    let first = lower
        .split(|character: char| !character.is_alphabetic())
        .find(|term| !term.is_empty());
    if first != Some("where") {
        return false;
    }
    let focus_terms = question_focus_terms(question, &[]);
    !focus_terms.is_empty()
        && focus_terms.iter().all(|term| {
            matches!(
                term.as_str(),
                "back" | "go" | "get" | "head" | "travel" | "visit" | "way" | "went"
            )
        })
}

fn source_question_has_unrequested_follow_up_modifier(question: &str, query: &str) -> bool {
    const FOLLOW_UP_MODIFIERS: &[&str] = &["other", "another", "additional", "more"];
    let words = |text: &str| {
        text.split(|character: char| !character.is_alphabetic())
            .filter(|word| !word.is_empty())
            .map(str::to_ascii_lowercase)
            .collect::<HashSet<_>>()
    };
    let query_words = words(query);
    let body = source_assertion_text(question)
        .split_once(':')
        .map_or(source_assertion_text(question), |(_, body)| body);
    source_question_clauses(body).into_iter().any(|clause| {
        let question_words = words(clause);
        FOLLOW_UP_MODIFIERS
            .iter()
            .any(|modifier| question_words.contains(*modifier) && !query_words.contains(*modifier))
    })
}

fn source_assertion_text(content: &str) -> &str {
    content
        .split_once("Upstream image metadata")
        .map_or(content, |(assertion, _)| assertion)
        .trim()
}

fn canonical_question_term(term: &str) -> String {
    if term.len() > 4 && term.ends_with("ed") {
        let stem = &term[..term.len() - 2];
        return if let Some(stem) = stem.strip_suffix('i') {
            format!("{stem}y")
        } else {
            stem.to_string()
        };
    }
    if term.len() > 3
        && term.ends_with('s')
        && !term.ends_with("ss")
        && !term.ends_with("us")
        && !term.ends_with("is")
    {
        term[..term.len() - 1].to_string()
    } else {
        term.to_string()
    }
}

fn question_focus_terms(query: &str, subject_terms: &[String]) -> HashSet<String> {
    let subjects = subject_terms
        .iter()
        .map(|term| canonical_question_term(term))
        .collect::<HashSet<_>>();
    crate::shared::search::search_query_terms(query)
        .into_iter()
        .map(|term| canonical_question_term(&term))
        .filter(|term| !QUESTION_FUNCTION_TERMS.contains(&term.as_str()))
        .filter(|term| !subjects.contains(term))
        .collect()
}

fn question_topic_overlap(question: &str, query: &str, subject_terms: &[String]) -> usize {
    let query_terms = question_focus_terms(query, subject_terms);
    let question_terms = question_focus_terms(question, &[]);
    query_terms.intersection(&question_terms).count()
}

fn source_question_clauses(body: &str) -> Vec<&str> {
    let mut questions = Vec::new();
    let mut clause_start = 0;
    for (index, character) in body.char_indices() {
        if character == '?' {
            questions.push(body[clause_start..=index].trim());
            clause_start = index + character.len_utf8();
        } else if matches!(character, '.' | '!' | '\n') {
            clause_start = index + character.len_utf8();
        }
    }
    questions
}

pub(super) fn source_question_contains_anaphoric_reference(content: &str) -> bool {
    let body = source_assertion_text(content)
        .split_once(':')
        .map_or(source_assertion_text(content), |(_, body)| body);
    source_question_clauses(body).into_iter().any(|question| {
        question
            .split(|character: char| !character.is_alphabetic())
            .any(|term| {
                matches!(
                    term.to_ascii_lowercase().as_str(),
                    "it" | "its"
                        | "this"
                        | "that"
                        | "one"
                        | "ones"
                        | "these"
                        | "those"
                        | "they"
                        | "them"
                        | "their"
                )
            })
    })
}

fn source_question_shares_personal_pronoun(question: &str, assertion: &str) -> bool {
    const PERSONAL_PRONOUNS: &[&str] = &[
        "he", "her", "hers", "him", "his", "it", "its", "she", "their", "them", "they",
    ];
    let pronouns = |text: &str| {
        let body = source_assertion_text(text)
            .split_once(':')
            .map_or(source_assertion_text(text), |(_, body)| body);
        body.split(|character: char| !character.is_alphabetic())
            .filter(|term| !term.is_empty())
            .map(str::to_ascii_lowercase)
            .filter(|term| PERSONAL_PRONOUNS.contains(&term.as_str()))
            .collect::<HashSet<_>>()
    };
    !pronouns(question).is_disjoint(&pronouns(assertion))
}

fn source_question_duration_topic_is_grounded(
    question: &str,
    query: &str,
    assertion: &str,
    subject_terms: &[String],
) -> bool {
    const DURATION_TERMS: &[&str] = &[
        "ago", "day", "long", "many", "month", "since", "time", "week", "year",
    ];
    let body = source_assertion_text(question)
        .split_once(':')
        .map_or(source_assertion_text(question), |(_, body)| body);
    let duration_clause = body
        .split([',', ';'])
        .flat_map(|segment| {
            let lower = segment.to_ascii_lowercase();
            let mut clauses = Vec::new();
            let mut start = 0;
            while let Some(relative_index) = lower[start..].find(" and ") {
                let split_index = start + relative_index;
                clauses.push(&segment[start..split_index]);
                start = split_index + " and ".len();
            }
            clauses.push(&segment[start..]);
            clauses
        })
        .find(|clause| asks_for_duration(clause))
        .unwrap_or(body);
    let duration_topics = question_focus_terms(duration_clause, subject_terms)
        .into_iter()
        .filter(|term| !DURATION_TERMS.contains(&term.as_str()))
        .collect::<HashSet<_>>();
    if duration_topics.is_empty() {
        return true;
    }

    let query_terms = question_focus_terms(query, subject_terms);
    let assertion_terms = crate::shared::search::search_query_terms(assertion)
        .into_iter()
        .map(|term| canonical_question_term(&term))
        .collect::<HashSet<_>>();
    duration_topics
        .iter()
        .any(|term| query_terms.contains(term) && assertion_terms.contains(term))
}

fn same_specific_interrogative(query: &str, question: &str) -> bool {
    fn family(text: &str) -> Option<&'static str> {
        let text = text
            .split_once(':')
            .map_or(text, |(_, body)| body)
            .to_ascii_lowercase();
        if asks_for_duration(&text) {
            return Some("duration");
        }
        if asks_for_calendar_date(&text) {
            return Some("date");
        }
        let text = text.trim_start();
        let first = text
            .split(|character: char| !character.is_alphabetic())
            .find(|token| !token.is_empty())?;
        match first {
            "where" => Some("where"),
            "when" => Some("when"),
            "who" | "whom" | "whose" => Some("who"),
            _ => None,
        }
    }

    family(query).is_some() && family(query) == family(question)
}

fn asks_for_duration(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("how long") || lower.contains("how many years")
}

fn asks_for_calendar_date(text: &str) -> bool {
    let lower = text.to_ascii_lowercase();
    lower.contains("when")
        || lower.contains("what year")
        || lower.contains("which year")
        || lower.contains("what date")
        || lower.contains("which date")
}

fn has_explicit_duration(text: &str) -> bool {
    let lower = source_assertion_text(text).to_ascii_lowercase();
    [
        " year", " years", " month", " months", " week", " weeks", " day", " days", " ago",
        "since ",
    ]
    .iter()
    .any(|marker| lower.contains(marker))
}

async fn preceding_source_context(
    ctx: &RetrievalContext,
    params: &super::params::DefaultContextParams<'_>,
    candidates: &[RankedContextFact],
) -> Result<HashMap<String, String>, MemoryError> {
    let episode_ids = candidates
        .iter()
        .map(|candidate| candidate.fact.source_episode.clone())
        .collect::<Vec<_>>();
    let window_start = params
        .window_start
        .as_ref()
        .map(|date| crate::shared::temporal::normalize_dt(*date));
    let window_end = params
        .window_end
        .as_ref()
        .map(|date| crate::shared::temporal::normalize_dt(*date));
    let records = ctx
        .episode_context_store()
        .select_source_neighborhoods_for_episodes(
            &episode_ids,
            params.cutoff_iso,
            window_start.as_deref(),
            window_end.as_deref(),
        )
        .await?;
    let episodes = super::filtering::filter_episodes_by_constraints(records, params.access);
    let query = params.raw_query_opt.unwrap_or_default();
    let query_evidence =
        super::admission::build_query_evidence(Some(query), Vec::new(), &[], params.query_flags);
    let subject_terms = query_evidence.subject_terms;
    let query_terms = question_focus_terms(query, &subject_terms);
    let required_answer_overlap = query_terms.len().min(2);
    let mut contexts = HashMap::new();

    for candidate in candidates {
        let Some(anchor) = episodes
            .iter()
            .find(|episode| episode.episode_id == candidate.fact.source_episode)
        else {
            continue;
        };
        let Some(source_context) = &anchor.source_context else {
            continue;
        };
        let mut context = String::new();
        let mut nearby = episodes
            .iter()
            .filter(|episode| {
                episode.source_context.as_ref().is_some_and(|source| {
                    source.group == source_context.group
                        && source.position != source_context.position
                        && source.position.abs_diff(source_context.position)
                            <= SOURCE_CONTEXT_RERANK_RADIUS
                })
            })
            .collect::<Vec<_>>();
        nearby.sort_by_key(|episode| {
            episode
                .source_context
                .as_ref()
                .map(|source| source.position)
        });
        for turn in nearby {
            let assertion = source_assertion_text(&turn.content);
            let immediate_question_answer = turn.source_context.as_ref().is_some_and(|source| {
                source.position.checked_add(1) == Some(source_context.position)
                    && !source_question_clauses(assertion).is_empty()
                    && (source_question_matches_query(assertion, query, &subject_terms)
                        || source_answer_mentions_query_slot(&anchor.content, query)
                        || !query_terms.is_empty()
                            && question_topic_overlap(&anchor.content, query, &subject_terms)
                                >= required_answer_overlap)
            });
            if assertion.is_empty()
                || (!source_context_is_related_support(query, &anchor.content, assertion)
                    && !immediate_question_answer)
            {
                continue;
            }
            let direction = if turn
                .source_context
                .as_ref()
                .is_some_and(|source| source.position < source_context.position)
            {
                "earlier"
            } else {
                "later"
            };
            context.push_str("Nearby source turn for context only (");
            context.push_str(direction);
            context.push_str("): ");
            context.push_str(assertion);
            context.push('\n');
        }

        if context.is_empty() {
            continue;
        }
        contexts.insert(anchor.episode_id.clone(), context);
    }

    Ok(contexts)
}

fn source_context_query_overlap(query: &str, source_context: &str) -> usize {
    let mut query_terms = question_focus_terms(query, &[]);
    query_terms.retain(|term| {
        !matches!(
            term.as_str(),
            "enjoy" | "watch" | "like" | "favorite" | "type" | "kind" | "main" | "hobby"
        )
    });
    let content = source_assertion_text(source_context)
        .split_once(':')
        .map_or(source_context, |(_, content)| content);
    let speaker_terms = source_context
        .split_once(':')
        .map(|(speaker, _)| crate::shared::search::search_query_terms(speaker))
        .unwrap_or_default();
    for term in speaker_terms {
        query_terms.remove(&term);
    }
    let content_terms = crate::shared::search::search_query_terms(content)
        .into_iter()
        .map(|term| canonical_question_term(&term))
        .collect::<HashSet<_>>();
    query_terms.intersection(&content_terms).count()
}

pub(super) fn source_question_is_supported_by_assertion(
    question: &str,
    query: &str,
    assertion: &str,
    subject_terms: &[String],
) -> bool {
    let question_body = source_assertion_text(question)
        .split_once(':')
        .map_or(source_assertion_text(question), |(_, body)| body);
    let assertion_body = source_assertion_text(assertion)
        .split_once(':')
        .map_or(source_assertion_text(assertion), |(_, body)| body);
    let assertion_terms = crate::shared::search::search_query_terms(assertion_body)
        .into_iter()
        .map(|term| canonical_question_term(&term))
        .collect::<HashSet<_>>();
    let clause_is_supported = |clause: &str| {
        let question_terms = question_focus_terms(clause, subject_terms);
        !question_terms.is_empty()
            && question_terms.intersection(&assertion_terms).count() >= question_terms.len().min(2)
    };
    if source_question_clauses(question_body)
        .into_iter()
        .any(|question_clause| {
            asks_for_duration(question_clause)
                && source_question_contains_anaphoric_reference(question_clause)
                && has_explicit_duration(assertion_body)
                && (clause_is_supported(question_clause)
                    || source_question_shares_personal_pronoun(question_clause, assertion_body)
                        && source_question_duration_topic_is_grounded(
                            question_clause,
                            query,
                            assertion_body,
                            subject_terms,
                        ))
        })
    {
        return true;
    }
    if source_question_clauses(question_body)
        .into_iter()
        .any(clause_is_supported)
    {
        return true;
    }

    // Anaphoric questions such as "Is this your first one?" can be grounded
    // by a preceding clause that names the object. Other preamble clauses do
    // not qualify a source answer to a different question.
    source_question_contains_anaphoric_reference(question) && clause_is_supported(question_body)
}

pub(super) fn anaphoric_response_asserts_requested_acquisition(
    query: &str,
    question: &str,
    response: &str,
) -> bool {
    let query_evidence = super::admission::build_query_evidence(
        Some(query),
        Vec::new(),
        &[],
        &super::query_mode::QueryFlags::default(),
    );
    super::admission::query_requests_acquisition(&query_evidence)
        && source_question_references_acquisition(question)
        && super::admission::content_asserts_acquisition(source_assertion_text(response))
}

fn source_question_references_acquisition(question: &str) -> bool {
    let body = source_assertion_text(question)
        .split_once(':')
        .map_or(source_assertion_text(question), |(_, body)| body);
    body.split(|character: char| !character.is_alphabetic())
        .filter(|term| !term.is_empty())
        .map(str::to_ascii_lowercase)
        .any(|term| {
            matches!(
                term.as_str(),
                "acquire"
                    | "acquired"
                    | "acquiring"
                    | "buy"
                    | "buys"
                    | "bought"
                    | "buying"
                    | "get"
                    | "gets"
                    | "got"
                    | "obtain"
                    | "obtains"
                    | "obtained"
                    | "obtaining"
                    | "own"
                    | "owns"
                    | "owned"
                    | "owning"
                    | "purchase"
                    | "purchases"
                    | "purchased"
                    | "purchasing"
            )
        })
}

pub(super) fn source_context_is_related_support(query: &str, answer: &str, support: &str) -> bool {
    let query_overlap = source_context_query_overlap(query, support);
    let temporal_query = same_specific_interrogative(query, "When did the event happen?");
    let answer_text = source_assertion_text(answer).to_ascii_lowercase();
    let has_duration_answer = has_explicit_duration(answer);
    let query_has_explicit_date = query
        .split_whitespace()
        .any(|token| token.chars().any(char::is_numeric));
    let assertion_terms = |content: &str| {
        let body = source_assertion_text(content)
            .split_once(':')
            .map_or(source_assertion_text(content), |(_, body)| body);
        crate::shared::search::search_query_terms(body)
            .into_iter()
            .map(|term| canonical_question_term(&term))
            .filter(|term| term.len() >= 4)
            .collect::<HashSet<_>>()
    };
    let answer_terms = assertion_terms(answer);
    let support_terms = assertion_terms(support);
    if source_context_explains_requested_transition(
        query,
        answer,
        support,
        &answer_terms,
        &support_terms,
    ) {
        return true;
    }
    if source_context_has_explicit_additive_link_with_terms(
        query,
        answer,
        support,
        &answer_terms,
        &support_terms,
    ) {
        return true;
    }

    let answer_has_anaphor = answer_text
        .split(|character: char| !character.is_alphabetic())
        .any(|term| {
            matches!(
                term,
                "it" | "its"
                    | "they"
                    | "them"
                    | "their"
                    | "he"
                    | "him"
                    | "his"
                    | "she"
                    | "her"
                    | "this"
                    | "these"
                    | "those"
                    | "one"
                    | "ones"
                    | "other"
                    | "another"
            )
        });
    let specific_query_link = query_overlap >= 2
        || ((temporal_query && has_duration_answer) || query_has_explicit_date)
            && query_overlap > 0;
    answer_has_anaphor && specific_query_link
}

pub(super) fn source_context_supports_requested_transition(
    query: &str,
    answer: &str,
    support: &str,
) -> bool {
    let assertion_terms = |content: &str| {
        let body = source_assertion_text(content)
            .split_once(':')
            .map_or(source_assertion_text(content), |(_, body)| body);
        crate::shared::search::search_query_terms(body)
            .into_iter()
            .map(|term| canonical_question_term(&term))
            .filter(|term| term.len() >= 4)
            .collect::<HashSet<_>>()
    };
    source_context_explains_requested_transition(
        query,
        answer,
        support,
        &assertion_terms(answer),
        &assertion_terms(support),
    )
}

pub(super) fn source_context_has_explicit_additive_link(
    query: &str,
    answer: &str,
    support: &str,
) -> bool {
    let terms = |content: &str| {
        let body = source_assertion_text(content)
            .split_once(':')
            .map_or(source_assertion_text(content), |(_, body)| body);
        crate::shared::search::search_query_terms(body)
            .into_iter()
            .map(|term| canonical_question_term(&term))
            .filter(|term| term.len() >= 4)
            .collect::<HashSet<_>>()
    };
    let answer_terms = terms(answer);
    let support_terms = terms(support);
    source_context_has_explicit_additive_link_with_terms(
        query,
        answer,
        support,
        &answer_terms,
        &support_terms,
    )
}

fn source_context_has_explicit_additive_link_with_terms(
    query: &str,
    answer: &str,
    support: &str,
    answer_terms: &HashSet<String>,
    support_terms: &HashSet<String>,
) -> bool {
    let query_terms = question_focus_terms(query, &[]);
    let shares_answer_topic = answer_terms
        .intersection(support_terms)
        .any(|term| term.len() >= 5 && !query_terms.contains(term));
    let answer_text = source_assertion_text(answer).to_ascii_lowercase();
    let answer_words = answer_text
        .split(|character: char| !character.is_alphabetic())
        .filter(|word| !word.is_empty())
        .collect::<HashSet<_>>();
    let answer_marks_additive_link = [
        "also",
        "too",
        "second",
        "another",
        "likewise",
        "similarly",
        "both",
    ]
    .iter()
    .any(|word| answer_words.contains(word))
        || ["same here", "as well", "me too"]
            .iter()
            .any(|phrase| answer_text.contains(phrase));

    shares_answer_topic
        && (answer_marks_additive_link
            || source_context_has_old_current_contrast(
                answer,
                support,
                answer_terms,
                support_terms,
            ))
}

fn source_context_explains_requested_transition(
    query: &str,
    answer: &str,
    support: &str,
    answer_terms: &HashSet<String>,
    support_terms: &HashSet<String>,
) -> bool {
    let terms = |text: &str| {
        text.split(|character: char| !character.is_alphanumeric())
            .filter(|term| !term.is_empty())
            .map(str::to_ascii_lowercase)
            .collect::<HashSet<_>>()
    };
    let query_terms = terms(query);
    let requests_transition = ["after", "before", "following", "since", "then"]
        .iter()
        .any(|term| query_terms.contains(*term));
    requests_transition
        && source_context_has_old_current_contrast(answer, support, answer_terms, support_terms)
}

fn source_context_has_old_current_contrast(
    answer: &str,
    support: &str,
    answer_terms: &HashSet<String>,
    support_terms: &HashSet<String>,
) -> bool {
    let terms = |text: &str| {
        text.split(|character: char| !character.is_alphanumeric())
            .filter(|term| !term.is_empty())
            .map(str::to_ascii_lowercase)
            .collect::<HashSet<_>>()
    };
    let answer_words = terms(answer);
    let support_words = terms(support);
    let old_markers = ["old", "previous", "former", "earlier"];
    let current_markers = ["new", "current", "latest", "replacement", "now"];
    let contrasts_old_and_current = old_markers.iter().any(|term| support_words.contains(*term))
        && current_markers
            .iter()
            .any(|term| answer_words.contains(*term))
        || current_markers
            .iter()
            .any(|term| support_words.contains(*term))
            && old_markers.iter().any(|term| answer_words.contains(*term));
    let shares_specific_subject = answer_terms
        .intersection(support_terms)
        .any(|term| term.len() >= 4);

    contrasts_old_and_current && shares_specific_subject
}

pub(super) fn source_context_is_travel_event_antecedent(
    query: &str,
    source_question: &str,
    antecedent: &str,
) -> bool {
    if !same_specific_interrogative(query, "Where did they go?") {
        return false;
    }
    let question_body = source_assertion_text(source_question)
        .split_once(':')
        .map_or(source_assertion_text(source_question), |(_, body)| body);
    if !source_question_clauses(question_body)
        .into_iter()
        .any(|clause| same_specific_interrogative(query, clause))
    {
        return false;
    }

    let query_terms = question_focus_terms(query, &[]);
    let antecedent_terms = question_focus_terms(antecedent, &[]);
    if query_terms.contains("family") && !antecedent_terms.contains("family") {
        return false;
    }
    [
        "getaway", "holiday", "journey", "roadtrip", "tour", "travel", "trip", "vacation",
    ]
    .iter()
    .any(|term| antecedent_terms.contains(*term))
}

pub(super) async fn filter_candidates(
    ctx: &RetrievalContext,
    params: &super::params::DefaultContextParams<'_>,
    candidates: Vec<RankedContextFact>,
) -> Result<Vec<RankedContextFact>, MemoryError> {
    let query = params.raw_query_opt.unwrap_or_default();
    let query_evidence =
        super::admission::build_query_evidence(Some(query), Vec::new(), &[], params.query_flags);
    let subject_terms = query_evidence.subject_terms.clone();
    let candidates = candidates
        .into_iter()
        .filter(|item| {
            let support = typed_answer_slot_support(query, &item.fact.content, &subject_terms);
            match support {
                Some(false) => false,
                Some(true) | None => true,
            }
        })
        .collect::<Vec<_>>();
    let source_context = if ctx.embedding_provider.reranking_signature().is_some() {
        preceding_source_context(ctx, params, &candidates).await?
    } else {
        HashMap::new()
    };
    let docs = candidates
        .iter()
        .map(|item| {
            document(
                &item.fact.content,
                Some(item.fact.t_valid),
                item.fact.provenance.source_id.as_deref().or(item
                    .fact
                    .provenance
                    .source_url
                    .as_deref()),
                source_context
                    .get(&item.fact.source_episode)
                    .map(String::as_str),
            )
        })
        .collect::<Vec<_>>();
    let Some(values) = scores_with_query_facets(ctx, query, &docs).await? else {
        return Ok(candidates);
    };
    let mut kept = candidates
        .into_iter()
        .zip(values)
        .filter_map(|(mut item, score)| {
            let floor = ctx.embedding_provider.reranking_admission_floor();
            let answer_slot_supported =
                typed_answer_slot_support(query, &item.fact.content, &subject_terms) == Some(true);
            let facet =
                matched_secondary_query_facet(query, &item.fact.content, &item.fact.index_keys);
            let facet_admitted =
                score < floor && !has_independent_admission_evidence(&item) && facet.is_some();
            if score < floor
                && !has_independent_admission_evidence(&item)
                && !facet_admitted
                && !answer_slot_supported
            {
                return None;
            }
            let mut reranking = evidence(ctx, score);
            if answer_slot_supported {
                reranking["answer_slot_supported"] = serde_json::Value::Bool(true);
            }
            if facet_admitted && let Some(facet) = facet {
                reranking["facet_admitted"] = serde_json::Value::Bool(true);
                reranking["matched_query_facet"] = serde_json::Value::String(facet);
            }
            item.evidence.reranking = Some(reranking);
            Some((score, item))
        })
        .collect::<Vec<_>>();
    kept.sort_by(|(a, left), (b, right)| {
        b.total_cmp(a)
            .then_with(|| left.fact.fact_id.cmp(&right.fact.fact_id))
    });
    Ok(kept.into_iter().map(|(_, item)| item).collect())
}

pub(super) async fn filter_items(
    ctx: &RetrievalContext,
    query: &str,
    items: Vec<AssembledContextItem>,
    budget: usize,
    preserve_input_order: bool,
) -> Result<Vec<AssembledContextItem>, MemoryError> {
    let query_evidence =
        super::admission::build_query_evidence(Some(query), Vec::new(), &[], &Default::default());
    let subject_terms = query_evidence.subject_terms.clone();
    let items = items
        .into_iter()
        .filter(|item| {
            let qualified_source_answer = item_has_qualified_source_episode_admission(item);
            match typed_answer_slot_support(query, &item.content, &subject_terms) {
                Some(true) => true,
                Some(false) if qualified_source_answer => true,
                Some(false) => false,
                None => true,
            }
        })
        .collect::<Vec<_>>();
    if ctx.embedding_provider.reranking_signature().is_none() || query.trim().is_empty() {
        return Ok(items.into_iter().take(budget).collect());
    }
    let indexed = items.into_iter().enumerate().collect::<Vec<_>>();
    let (already, pending): (Vec<_>, Vec<_>) = indexed.into_iter().partition(|(_, item)| {
        item.provenance
            .get("reranking")
            .and_then(|e| e.get("model_signature"))
            .and_then(serde_json::Value::as_str)
            == ctx.embedding_provider.reranking_signature()
    });
    let mut scored = Vec::with_capacity(already.len() + pending.len());
    for (input_position, item) in already {
        let score = item
            .provenance
            .get("reranking")
            .and_then(|evidence| evidence.get("relevance_logit"))
            .and_then(serde_json::Value::as_f64)
            .filter(|score| score.is_finite())
            .ok_or_else(|| {
                MemoryError::Validation(
                    "cached reranking evidence has no finite relevance logit".into(),
                )
            })?;
        if score >= ctx.embedding_provider.reranking_admission_floor()
            || item_has_lexical_subject_support(&item, query)
            || item_has_exact_reference_admission(&item)
            || item_has_calendar_acquisition_admission(&item)
            || item_has_qualified_source_episode_admission(&item)
            || item_has_explicit_date_admission(&item)
            || item_has_secondary_facet_admission(&item)
        {
            scored.push((score, input_position, item));
        }
    }
    let docs = pending
        .iter()
        .map(|(_, item)| {
            document(
                &item.content,
                item.provenance
                    .get("reference_time")
                    .and_then(serde_json::Value::as_str)
                    .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
                    .map(|date| date.with_timezone(&chrono::Utc)),
                item.provenance
                    .get("source_id")
                    .and_then(serde_json::Value::as_str),
                None,
            )
        })
        .collect::<Vec<_>>();
    if let Some(values) = scores_with_query_facets(ctx, query, &docs).await? {
        for ((input_position, mut item), score) in pending.into_iter().zip(values) {
            let facet = matched_secondary_query_facet(query, &item.content, &[]);
            let facet_admitted =
                score < ctx.embedding_provider.reranking_admission_floor() && facet.is_some();
            if score < ctx.embedding_provider.reranking_admission_floor()
                && !item_has_lexical_subject_support(&item, query)
                && !item_has_exact_reference_admission(&item)
                && !item_has_calendar_acquisition_admission(&item)
                && !item_has_qualified_source_episode_admission(&item)
                && !item_has_explicit_date_admission(&item)
                && !item_has_secondary_facet_admission(&item)
                && !facet_admitted
            {
                continue;
            }
            if !item.provenance.is_object() {
                item.provenance = serde_json::json!({});
            }
            let mut reranking = evidence(ctx, score);
            if facet_admitted && let Some(facet) = facet {
                reranking["facet_admitted"] = serde_json::Value::Bool(true);
                reranking["matched_query_facet"] = serde_json::Value::String(facet);
            }
            item.provenance["reranking"] = reranking;
            scored.push((score, input_position, item));
        }
    }
    if preserve_input_order {
        scored.sort_by_key(|(_, input_position, _)| *input_position);
    } else {
        scored.sort_by(|(left_score, _, left_item), (right_score, _, right_item)| {
            right_score
                .total_cmp(left_score)
                .then_with(|| left_item.fact_id.cmp(&right_item.fact_id))
        });
    }
    Ok(scored
        .into_iter()
        .take(budget)
        .map(|(_, _, item)| item)
        .collect())
}

pub(super) fn item_has_direct_lexical_evidence(item: &AssembledContextItem) -> bool {
    item.retrieval_tier.as_deref() == Some("direct") && item_has_lexical_admission_evidence(item)
}

fn item_has_lexical_admission_evidence(item: &AssembledContextItem) -> bool {
    item.provenance
        .get("retrieval_admission")
        .and_then(|admission| admission.get("basis"))
        .and_then(serde_json::Value::as_str)
        == Some("lexical")
        && item
            .provenance
            .get("retrieval_admission")
            .and_then(|admission| admission.get("matched_content_terms"))
            .and_then(serde_json::Value::as_array)
            .is_some_and(|terms| !terms.is_empty())
}

fn item_has_lexical_subject_support(item: &AssembledContextItem, query: &str) -> bool {
    item_has_lexical_admission_evidence(item)
        && super::admission::candidate_has_explicit_subject_support(query, &item.content)
}

fn item_has_secondary_facet_admission(item: &AssembledContextItem) -> bool {
    item.provenance
        .get("reranking")
        .and_then(|evidence| evidence.get("facet_admitted"))
        .and_then(serde_json::Value::as_bool)
        .unwrap_or(false)
        && item
            .provenance
            .get("reranking")
            .and_then(|evidence| evidence.get("matched_query_facet"))
            .and_then(serde_json::Value::as_str)
            .is_some_and(|facet| !facet.trim().is_empty())
}
