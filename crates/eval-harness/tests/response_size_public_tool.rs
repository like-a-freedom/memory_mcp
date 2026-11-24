use eval_harness::test_support::{TestMemory, ingest_probe};
use memory_mcp::tools::{explain, params::ExplainParams};
use serde_json::json;

async fn assert_bounded_explanation(compact: bool) {
    let memory = TestMemory::new().await;
    let quote = "Exact small quote.";
    let source = format!("{quote} {}", "\"\\é🙂 ".repeat(1800));
    let episode = ingest_probe(&memory.service, "escaping-heavy", &source).await;
    let response = explain(
        &memory.service,
        ExplainParams {
            context_items: json!([{"source_episode": episode, "quote": quote}]).to_string(),
            compact,
            ..ExplainParams::default()
        },
    )
    .await
    .unwrap();

    let bytes = serde_json::to_vec(&response).unwrap().len();
    assert!(bytes <= 32_768, "compact={compact} returned {bytes} bytes");
    assert_eq!(response.result[0]["quote"], quote);
}

#[tokio::test]
async fn explain_public_envelope_fits_32768_bytes_in_compact_mode() {
    assert_bounded_explanation(true).await;
}

#[tokio::test]
async fn explain_public_envelope_fits_32768_bytes_in_verbose_mode() {
    assert_bounded_explanation(false).await;
}

#[tokio::test]
async fn explain_source_continuations_reconstruct_original_unicode() {
    let memory = TestMemory::new().await;
    let source = "é\u{301}🙂\"\\".repeat(33);
    let episode = ingest_probe(&memory.service, "unicode-source", &source).await;
    let mut offset = 0;
    let mut reconstructed = String::new();

    loop {
        let response = explain(
            &memory.service,
            serde_json::from_value(json!({
                "context_items": json!([episode]).to_string(),
                "source_offset": offset,
                "source_limit_scalars": 13,
            }))
            .unwrap(),
        )
        .await
        .unwrap();
        let item = &response.result[0];
        assert_eq!(item["citation_context_start_offset"], offset);
        reconstructed.push_str(item["citation_context"].as_str().unwrap());
        let Some(next) = item["citation_context_next_offset"].as_u64() else {
            break;
        };
        assert!(next > offset);
        assert!(next < source.chars().count() as u64);
        offset = next;
    }
    assert_eq!(reconstructed, source);
}

#[tokio::test]
async fn explicit_source_range_keeps_a_fitting_quote_when_the_window_must_shrink() {
    let memory = TestMemory::new().await;
    let quote = "precise quotation ".repeat(1400);
    let episode = ingest_probe(&memory.service, "large-verified-quote", &quote).await;
    let response = explain(
        &memory.service,
        ExplainParams {
            context_items: json!([{"source_episode": episode, "quote": quote}]).to_string(),
            source_offset: Some(0),
            ..ExplainParams::default()
        },
    )
    .await
    .unwrap();

    assert!(serde_json::to_vec(&response).unwrap().len() <= 32_768);
    let item = &response.result[0];
    assert_eq!(item["quote"], quote);
    assert_eq!(item["quote_verified"], true);
    let window = item["citation_context"].as_str().unwrap();
    assert!(!window.is_empty());
    assert_eq!(item["citation_context_next_offset"], window.chars().count());
    assert!(
        response
            .partial_reasons
            .contains(&"response_budget_applied")
    );
}

#[tokio::test]
async fn metadata_only_provenance_does_not_repeat_source_bodies() {
    let memory = TestMemory::new().await;
    let episode = ingest_probe(&memory.service, "metadata-only", "A short source.").await;
    let response = explain(
        &memory.service,
        ExplainParams {
            context_items: json!([episode]).to_string(),
            provenance_offset: Some(0),
            ..ExplainParams::default()
        },
    )
    .await
    .unwrap();

    let item = &response.result[0];
    assert_eq!(item["provenance_total_count"], 1);
    assert_eq!(item["all_sources"][0]["episode_id"], episode);
    assert!(item["all_sources"][0].get("episode_content").is_none());
}
