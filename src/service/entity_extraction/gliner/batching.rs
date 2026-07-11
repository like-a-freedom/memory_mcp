use std::ops::Range;

#[derive(Debug, Clone)]
pub(super) struct EncodedWindow {
    pub(super) input_ids: Vec<u32>,
    pub(super) word_ids: Vec<Option<u32>>,
    pub(super) window_start: usize,
    pub(super) window_end: usize,
}

pub(super) fn pack_window_batches(
    windows: &[EncodedWindow],
    max_windows: usize,
    max_padded_tokens: usize,
) -> Vec<Range<usize>> {
    let mut batches = Vec::new();
    let mut start = 0;
    while start < windows.len() {
        let mut end = start;
        let mut longest = 0;
        while end < windows.len() && end - start < max_windows {
            let candidate_longest = longest.max(windows[end].input_ids.len());
            let candidate_count = end - start + 1;
            if candidate_count > 1 && candidate_longest * candidate_count > max_padded_tokens {
                break;
            }
            longest = candidate_longest;
            end += 1;
        }
        batches.push(start..end.max(start + 1));
        start = end.max(start + 1);
    }
    batches
}

#[cfg(test)]
mod tests {
    use super::*;

    fn window(tokens: usize) -> EncodedWindow {
        EncodedWindow {
            input_ids: vec![1; tokens],
            word_ids: vec![Some(0); tokens],
            window_start: 0,
            window_end: tokens,
        }
    }

    #[test]
    fn respects_window_and_padded_token_limits() {
        let windows = vec![window(100), window(120), window(300), window(300)];
        assert_eq!(
            pack_window_batches(&windows, 4, 480),
            vec![0..2, 2..3, 3..4]
        );
    }

    #[test]
    fn always_makes_progress_for_one_oversized_window() {
        let windows = vec![window(600)];
        assert_eq!(pack_window_batches(&windows, 4, 384), vec![0..1]);
    }

    #[test]
    #[ignore = "requires local GLiNER model files under tests/models/ner/urchade--gliner_multi-v2.1"]
    fn batched_forward_matches_unbatched_hidden_states_with_padding() {
        // Batched f32 GEMM/softmax kernels use different reduction shapes from
        // batch=1. Keep this CPU diagnostic tight while treating exact decoded
        // candidates as the hard quality gate.
        const ATOL: f32 = 5e-5;
        const RTOL: f32 = 1e-4;

        let model_dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("tests/models/ner/urchade--gliner_multi-v2.1");
        let labels = vec![
            "person".into(),
            "company".into(),
            "location".into(),
            "product".into(),
            "event".into(),
            "technology".into(),
        ];
        let extractor = crate::service::entity_extraction::gliner::GlinerEntityExtractor::new(
            &model_dir,
            labels.clone(),
            crate::config::DEFAULT_NER_THRESHOLD,
        )
        .expect("load local GLiNER model");
        let prompt_words = extractor.build_prompt_words_for_labels(&labels);

        let encode = |text: &str| {
            let text_words =
                crate::service::entity_extraction::gliner::GlinerEntityExtractor::split_text_words(
                    text,
                );
            let (encoding, window_end) = extractor
                .encode_window(&prompt_words, &text_words, 0)
                .expect("encode GLiNER window");
            EncodedWindow {
                input_ids: encoding.get_ids().to_vec(),
                word_ids: encoding.get_word_ids().to_vec(),
                window_start: 0,
                window_end,
            }
        };

        let short = encode("Alice Smith joined OpenAI in Moscow.");
        let long = encode(
            "Alice Smith joined OpenAI in Moscow and presented Project Atlas using Rust, Kubernetes, PostgreSQL, and Candle at the annual engineering summit.",
        );
        assert!(short.input_ids.len() < long.input_ids.len());

        let unbatched = extractor
            .run_forward(&short.input_ids)
            .expect("unbatched forward")
            .to_vec2::<f32>()
            .expect("unbatched hidden values");
        let padded_short = extractor
            .run_forward_batch(&[short, long])
            .expect("batched forward")
            .remove(0)
            .to_vec2::<f32>()
            .expect("batched hidden values");

        assert_eq!(unbatched.len(), padded_short.len());
        assert_eq!(
            unbatched.first().map(Vec::len),
            padded_short.first().map(Vec::len)
        );
        let mut worst_mismatch = None;
        for (index, (expected, actual)) in unbatched
            .iter()
            .flatten()
            .zip(padded_short.iter().flatten())
            .enumerate()
        {
            assert!(expected.is_finite() && actual.is_finite());
            let tolerance = ATOL + RTOL * expected.abs();
            let difference = (expected - actual).abs();
            if difference > tolerance
                && worst_mismatch
                    .as_ref()
                    .is_none_or(|(_, _, _, _, worst_ratio)| difference / tolerance > *worst_ratio)
            {
                worst_mismatch = Some((
                    index,
                    *expected,
                    *actual,
                    difference,
                    difference / tolerance,
                ));
            }
        }
        assert!(
            worst_mismatch.is_none(),
            "hidden-state mismatch beyond atol={ATOL} rtol={RTOL}: {worst_mismatch:?}"
        );
    }
}
