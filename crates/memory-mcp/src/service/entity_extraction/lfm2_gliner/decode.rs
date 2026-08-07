//! Pure span-decoding helpers for the SauerkrautLM LFM2 GLiNER backend
//! (Task 9).
//!
//! Everything in this module is model-free: word splitting, span
//! enumeration, prompt token-position collection, thresholded span
//! extraction, and non-maximum suppression. The tensor-level head math lives
//! in [`super::model`]; this module feeds it text shapes and consumes its
//! score rows.

use std::sync::LazyLock;

/// Whitespace/punctuation word splitter matching the classic GLiNER backend
/// (`\w+(?:[-_]\w+)*|\S`). Yields byte offsets, so `&text[start..end]`
/// round-trips for any UTF-8 input.
static WHITESPACE_WORD_SPLITTER: LazyLock<regex::Regex> =
    LazyLock::new(|| regex::Regex::new(r"\w+(?:[-_]\w+)*|\S").expect("valid splitter regex"));

/// Splits `text` into words with byte offsets (classic `split_text_words`).
pub(crate) fn split_text_words(text: &str) -> Vec<(String, (usize, usize))> {
    WHITESPACE_WORD_SPLITTER
        .find_iter(text)
        .map(|mat| (mat.as_str().to_string(), (mat.start(), mat.end())))
        .collect()
}

/// Enumerates candidate spans in the upstream `prepare_span_idx` order:
/// start-major, width-minor, end inclusive, restricted to valid ends
/// (`end < text_len`). For `text_len = 4, max_span_width = 2` this yields
/// `[(0,0),(0,1),(1,1),(1,2),(2,2),(2,3),(3,3)]` — the same valid span set the
/// upstream decoder emits (its 8th `(3,4)` row is masked out at decode time).
pub(crate) fn enumerate_span_indices(
    text_len: usize,
    max_span_width: usize,
) -> Vec<(usize, usize)> {
    let mut indices = Vec::new();
    for start in 0..text_len {
        for end in start..text_len.min(start + max_span_width) {
            indices.push((start, end));
        }
    }
    indices
}

/// Logistic sigmoid over a raw logit.
pub(crate) fn sigmoid(logit: f32) -> f32 {
    1.0 / (1.0 + (-logit).exp())
}

/// Collects token positions whose id equals `ent_token_id` and whose word id
/// falls inside the prompt (word id `< prompt_word_count`). The upstream
/// `extract_prompt_features` gathers rows by `input_ids == class_token_index`;
/// for this checkpoint `class_token_index == ent_token_id`, so matching the
/// marker token id is equivalent.
pub(crate) fn collect_ent_token_positions(
    input_ids: &[u32],
    word_ids: &[Option<u32>],
    ent_token_id: u32,
    prompt_word_count: usize,
) -> Vec<usize> {
    input_ids
        .iter()
        .enumerate()
        .filter_map(|(index, token_id)| {
            (token_id == &ent_token_id
                && word_ids
                    .get(index)
                    .and_then(|word_id| *word_id)
                    .is_some_and(|word_id| word_id < prompt_word_count as u32))
            .then_some(index)
        })
        .collect()
}

/// A decoded entity span. `start`/`end` are BYTE offsets into the source
/// text (from the word splitter), so `&text[start..end]` round-trips.
///
/// `#[doc(hidden)] pub` so the release-parity integration test can compare
/// native scores against the Python reference; not part of the public API.
#[doc(hidden)]
#[derive(Debug, Clone, PartialEq)]
pub struct ScoredEntity {
    pub start: usize,
    pub end: usize,
    pub text: String,
    pub label: String,
    pub score: f32,
}

/// Converts per-span score rows into thresholded [`ScoredEntity`] instances.
///
/// `spans_data` is `(start_word, end_word, logits_per_label)` in enumerate
/// order; `offsets` are the per-word byte offsets. Logits are sigmoided and
/// accepted when the probability meets `threshold`. Spans whose end falls
/// outside `offsets` or whose trimmed text is empty are skipped.
pub(crate) fn extract_spans(
    text: &str,
    spans_data: &[(usize, usize, Vec<f32>)],
    offsets: &[(usize, usize)],
    labels: &[String],
    threshold: f64,
) -> Vec<ScoredEntity> {
    let mut spans = Vec::new();

    for &(start, end, ref scores) in spans_data {
        if start >= offsets.len() || end >= offsets.len() {
            continue;
        }

        let start_char = offsets[start].0;
        let end_char = offsets[end].1;
        if end_char <= start_char || end_char > text.len() {
            continue;
        }

        let span_text = text[start_char..end_char].trim();
        if span_text.is_empty() {
            continue;
        }

        for (label_idx, &score) in scores.iter().enumerate() {
            if label_idx >= labels.len() {
                break;
            }
            let probability = sigmoid(score);
            if probability >= threshold as f32 {
                spans.push(ScoredEntity {
                    start: start_char,
                    end: end_char,
                    text: span_text.to_string(),
                    label: labels[label_idx].clone(),
                    score: probability,
                });
            }
        }
    }

    spans
}

/// Greedy non-maximum suppression over same-label spans with IOU > 0.5,
/// keeping the highest-scoring survivor (classic `apply_nms`).
pub(crate) fn apply_nms(mut spans: Vec<ScoredEntity>) -> Vec<ScoredEntity> {
    const IOU_THRESHOLD: f32 = 0.5;

    spans.sort_by(|left, right| {
        right
            .score
            .partial_cmp(&left.score)
            .unwrap_or(std::cmp::Ordering::Equal)
    });

    let mut kept = Vec::new();
    for span in spans {
        let dominated = kept.iter().any(|kept_span: &ScoredEntity| {
            if kept_span.label != span.label {
                return false;
            }
            let inter_start = span.start.max(kept_span.start);
            let inter_end = span.end.min(kept_span.end);
            if inter_start >= inter_end {
                return false;
            }
            let intersection = (inter_end - inter_start) as f32;
            let union =
                (span.end - span.start + kept_span.end - kept_span.start) as f32 - intersection;
            intersection / union > IOU_THRESHOLD
        });

        if !dominated {
            kept.push(span);
        }
    }

    kept
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn span_enumeration_matches_prepare_span_idx_ordering() {
        // Upstream `prepare_span_idx(4, 2)` emits 8 rows; the invalid
        // `(3, 4)` end is masked at decode time, leaving exactly these 7.
        assert_eq!(
            enumerate_span_indices(4, 2),
            vec![(0, 0), (0, 1), (1, 1), (1, 2), (2, 2), (2, 3), (3, 3),]
        );
    }

    #[test]
    fn span_enumeration_clamps_width_at_sequence_end() {
        assert_eq!(enumerate_span_indices(2, 5), vec![(0, 0), (0, 1), (1, 1)]);
    }

    #[test]
    fn empty_text_has_no_spans() {
        assert!(enumerate_span_indices(0, 12).is_empty());
    }

    #[test]
    fn sigmoid_maps_zero_logit_to_half() {
        assert!((sigmoid(0.0) - 0.5).abs() < 1e-6);
        assert!(sigmoid(2.0) > 0.8);
        assert!(sigmoid(-2.0) < 0.2);
    }

    #[test]
    fn ent_token_positions_are_collected_with_prompt_word_filter() {
        let input_ids = [7u32, 30, 8, 30, 9, 10, 30];
        let word_ids = [
            Some(0),
            Some(0),
            Some(1),
            Some(2),
            Some(2),
            Some(3),
            Some(3),
        ];
        // `<<ENT>>` (30) appears at prompt words 0 and 2 (in-prompt) and at
        // word 3 (text word 0) — the last one must be excluded.
        assert_eq!(
            collect_ent_token_positions(&input_ids, &word_ids, 30, 3),
            vec![1, 3]
        );
    }

    #[test]
    fn extract_spans_uses_byte_offsets_for_cyrillic_text() {
        let text = "Привет, Алексей";
        // Regex split: "Привет" (0..12), "," (12..13), "Алексей" (14..28).
        let offsets = [(0, 12), (12, 13), (14, 28)];
        let labels = ["person".to_string(), "company".to_string()];
        let spans_data = [
            (0usize, 0usize, vec![2.0_f32, -2.0]),
            (0, 2, vec![2.0, -2.0]),
            (1, 1, vec![-1.0, -1.0]),
        ];

        let spans = extract_spans(text, &spans_data, &offsets, &labels, 0.5);

        assert_eq!(spans.len(), 2, "sigmoid(-2) must be below threshold");
        assert_eq!(spans[0].start, 0);
        assert_eq!(spans[0].end, 12);
        assert_eq!(spans[0].text, "Привет");
        assert_eq!(&text[spans[0].start..spans[0].end], "Привет");
        assert_eq!(spans[0].label, "person");

        assert_eq!(spans[1].start, 0);
        assert_eq!(spans[1].end, 28);
        assert_eq!(spans[1].text, "Привет, Алексей");
        assert_eq!(&text[spans[1].start..spans[1].end], "Привет, Алексей");
    }

    #[test]
    fn extract_spans_round_trips_mixed_cyrillic_latin_text() {
        let text = "Alice met Иван at Acme";
        // "Alice"(0..5) "met"(6..9) "Иван"(10..18) "at"(19..21) "Acme"(22..26)
        let offsets = [(0, 5), (6, 9), (10, 18), (19, 21), (22, 26)];
        let labels = ["person".to_string()];
        let spans_data = [
            (2usize, 2usize, vec![3.0_f32]),
            (4, 4, vec![3.0]),
            (0, 4, vec![3.0]),
        ];

        let spans = extract_spans(text, &spans_data, &offsets, &labels, 0.5);

        assert_eq!(spans.len(), 3);
        for span in &spans {
            assert_eq!(&text[span.start..span.end], span.text);
        }
        assert_eq!(spans[0].text, "Иван");
        assert_eq!(spans[2].text, "Alice met Иван at Acme");
    }

    #[test]
    fn extract_spans_skips_invalid_and_empty_spans() {
        let text = "a  b";
        let offsets = [(0, 1), (3, 4)];
        let labels = ["person".to_string()];
        // End out of range (index 5) and a span whose trimmed text is empty.
        let spans_data = [(0usize, 5usize, vec![5.0_f32]), (1, 1, vec![5.0])];
        let spans = extract_spans(text, &spans_data, &offsets, &labels, 0.5);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].text, "b");
    }

    #[test]
    fn extract_spans_respects_threshold_per_label() {
        let text = "Alice Smith";
        let offsets = [(0, 5), (6, 11)];
        let labels = ["person".to_string(), "company".to_string()];
        let spans_data = [(0usize, 1usize, vec![2.0_f32, 0.0])];
        // threshold 0.7: sigmoid(2.0)=0.88 accepted, sigmoid(0.0)=0.5 rejected.
        let spans = extract_spans(text, &spans_data, &offsets, &labels, 0.7);
        assert_eq!(spans.len(), 1);
        assert_eq!(spans[0].label, "person");
        assert_eq!(spans[0].text, "Alice Smith");
    }

    #[test]
    fn apply_nms_keeps_one_survivor_per_same_label_overlap() {
        let spans = vec![
            ScoredEntity {
                start: 0,
                end: 10,
                text: "full".to_string(),
                label: "person".to_string(),
                score: 0.6,
            },
            ScoredEntity {
                start: 1,
                end: 9,
                text: "inner".to_string(),
                label: "person".to_string(),
                score: 0.9,
            },
            ScoredEntity {
                start: 0,
                end: 10,
                text: "full".to_string(),
                label: "company".to_string(),
                score: 0.8,
            },
        ];
        let kept = apply_nms(spans);
        // The high-scoring inner person span dominates the lower one; the
        // company span (different label) survives alongside it.
        assert_eq!(kept.len(), 2);
        assert!(kept.iter().any(|s| s.label == "company"));
        assert!(
            kept.iter()
                .any(|s| s.label == "person" && s.text == "inner")
        );
    }

    #[test]
    fn split_text_words_yields_byte_offsets() {
        let words = split_text_words("Привет, Алексей!");
        assert_eq!(
            words,
            vec![
                ("Привет".to_string(), (0, 12)),
                (",".to_string(), (12, 13)),
                ("Алексей".to_string(), (14, 28)),
                ("!".to_string(), (28, 29)),
            ]
        );
    }
}
