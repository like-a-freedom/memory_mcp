use super::TextChunk;

const DEFAULT_CHUNK_WORDS: usize = 400;
const DEFAULT_CHUNK_OVERLAP_WORDS: usize = 50;

pub(crate) fn chunk_text(chunks: Vec<TextChunk>) -> Vec<TextChunk> {
    let mut expanded = Vec::new();

    for chunk in chunks {
        let words = chunk
            .content
            .split_whitespace()
            .map(ToString::to_string)
            .collect::<Vec<_>>();
        if words.is_empty() {
            continue;
        }

        if words.len() <= DEFAULT_CHUNK_WORDS {
            expanded.push(TextChunk {
                label: chunk.label,
                content: words.join(" "),
            });
            continue;
        }

        let mut start = 0;
        while start < words.len() {
            let end = (start + DEFAULT_CHUNK_WORDS).min(words.len());
            expanded.push(TextChunk {
                label: chunk.label.clone(),
                content: words[start..end].join(" "),
            });
            if end == words.len() {
                break;
            }
            start = end.saturating_sub(DEFAULT_CHUNK_OVERLAP_WORDS);
        }
    }

    let total = expanded.len();
    expanded
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| {
            let prefix = format!("Chunk {}/{}", index + 1, total.max(1));
            let label = match chunk.label {
                Some(label) if !label.trim().is_empty() => Some(format!("{prefix} · {label}")),
                _ => Some(prefix),
            };
            TextChunk {
                label,
                content: chunk.content,
            }
        })
        .collect()
}

pub(crate) fn render_chunks(chunks: &[TextChunk]) -> String {
    chunks
        .iter()
        .map(|chunk| match chunk.label.as_deref() {
            Some(label) if !label.trim().is_empty() => format!("{label}\n{}", chunk.content),
            _ => chunk.content.clone(),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_text_splits_large_inputs_with_overlap() {
        let source = TextChunk {
            label: None,
            content: (1..=450)
                .map(|index| format!("w{index:04}"))
                .collect::<Vec<_>>()
                .join(" "),
        };

        let chunked = chunk_text(vec![source]);

        assert_eq!(chunked.len(), 2);
        assert_eq!(chunked[0].label.as_deref(), Some("Chunk 1/2"));
        assert_eq!(chunked[1].label.as_deref(), Some("Chunk 2/2"));
        assert!(chunked[1].content.starts_with("w0351 w0352 w0353"));
    }

    #[test]
    fn render_chunks_emits_labels_and_spacing() {
        let rendered = render_chunks(&[
            TextChunk {
                label: Some("Chunk 1/2".to_string()),
                content: "alpha beta".to_string(),
            },
            TextChunk {
                label: Some("Chunk 2/2".to_string()),
                content: "gamma delta".to_string(),
            },
        ]);

        assert!(rendered.contains("Chunk 1/2\nalpha beta"));
        assert!(rendered.contains("Chunk 2/2\ngamma delta"));
        assert!(rendered.contains("\n\nChunk 2/2"));
    }
}
