#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct SpanIndex {
    pub(super) start: usize,
    pub(super) end: usize,
}

pub(super) fn enumerate_span_indices(text_len: usize, max_span_width: usize) -> Vec<SpanIndex> {
    let mut indices = Vec::new();
    for start in 0..text_len {
        for end in start..std::cmp::min(start + max_span_width, text_len) {
            indices.push(SpanIndex { start, end });
        }
    }
    indices
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn enumerates_the_same_inclusive_spans_as_the_reference_loop() {
        assert_eq!(
            enumerate_span_indices(4, 2),
            vec![
                SpanIndex { start: 0, end: 0 },
                SpanIndex { start: 0, end: 1 },
                SpanIndex { start: 1, end: 1 },
                SpanIndex { start: 1, end: 2 },
                SpanIndex { start: 2, end: 2 },
                SpanIndex { start: 2, end: 3 },
                SpanIndex { start: 3, end: 3 },
            ]
        );
    }

    #[test]
    fn empty_text_has_no_spans() {
        assert!(enumerate_span_indices(0, 12).is_empty());
    }
}
