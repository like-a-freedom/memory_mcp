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
    #[ignore = "requires GLiNER model weights — run with --ignored"]
    fn batched_forward_matches_unbatched_hidden_states_with_padding() {
        let model_dir = std::path::PathBuf::from(
            std::env::var("GLINER_MODEL_DIR").unwrap_or_else(|_| "models/nicegui_default".into()),
        );
        let extractor = crate::service::entity_extraction::gliner::GlinerEntityExtractor::new(
            &model_dir,
            vec!["person".into(), "organization".into(), "location".into()],
            crate::config::DEFAULT_NER_THRESHOLD,
        )
        .unwrap();
        let text = "Alice Johnson works at OpenAI in San Francisco and previously joined Microsoft Research.";
        let labels = vec!["person".into(), "organization".into(), "location".into()];
        let prompt_words = extractor.build_prompt_words_for_labels(&labels);
        let _prompt_word_count = prompt_words.len();
        let text_words =
            crate::service::entity_extraction::gliner::GlinerEntityExtractor::split_text_words(
                text,
            );
        let mut windows = Vec::new();
        let mut window_start = 0;
        while window_start < text_words.len() {
            let (encoding, window_end) = extractor
                .encode_window(&prompt_words, &text_words, window_start)
                .unwrap();
            windows.push(EncodedWindow {
                input_ids: encoding.get_ids().to_vec(),
                word_ids: encoding.get_word_ids().to_vec(),
                window_start,
                window_end,
            });
            if window_end >= text_words.len() {
                break;
            }
            window_start = window_end.saturating_sub(1).max(window_start + 1);
        }
        assert!(!windows.is_empty());
        let sample_indices: Vec<usize> = (0..windows.len())
            .step_by((windows.len() / 3).max(1))
            .take(3)
            .collect();
        for idx in sample_indices {
            let unbatched = extractor.run_forward(&windows[idx].input_ids).unwrap();
            let batched = extractor
                .run_forward_batch(&[windows[idx].clone()])
                .unwrap();
            assert_eq!(unbatched.dims(), batched[0].dims());
            let unbatched_vec = unbatched.to_vec2::<f32>().unwrap();
            let batched_vec = batched[0].to_vec2::<f32>().unwrap();
            let mut max_diff = f32::NEG_INFINITY;
            for (u_row, b_row) in unbatched_vec.iter().zip(batched_vec.iter()) {
                for (u_val, b_val) in u_row.iter().zip(b_row.iter()) {
                    let d = (u_val - b_val).abs();
                    if d > max_diff {
                        max_diff = d;
                    }
                }
            }
            assert!(
                max_diff <= 1e-5,
                "hidden state max diff {max_diff} exceeded atol 1e-5 at window {idx}"
            );
        }
    }
}
