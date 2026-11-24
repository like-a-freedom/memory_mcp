# Bounded `explain` Citation Output — Design

**Status:** Proposed — awaiting approval  
**Date:** 2026-11-13  
**Scope:** Bound every successful `explain` response while preserving exact citation evidence and on-demand access to complete source text. This document specifies behavior only; it does not authorize implementation until approved.

## Implementation debt — 2026-11-24

This proposal remains pending. The current adapter has no `explain_budget` module, source/provenance continuation contract or public 32 KiB enforcement proof. Implementation requires the existing `explain` public interface tests to reproduce unbounded output, then capability-owned quote/window/provenance semantics and adapter-owned final-envelope budgeting. Acceptance must cover both modes, escaping-heavy and multi-megabyte inputs, exact quote retention, distinct item/source/provenance offsets, atomic partial metadata and exact Unicode-scalar reconstruction. Extend the response-size evaluator through the real tool envelope and retain the 61 shared cases plus a separately named synthetic reconstruction case.

Ratify the compatibility change in a newly allocated ADR and amend ADR-0022 and the July 30 response specification. ADR-0083 now belongs to [revision-bounded temporal context](../../adr/0083-revision-bounded-temporal-context.md), so the historical ADR number proposed below is superseded. Current implementation must follow the workspace MSRV and repository-required feature/test checks.

## Intent

Prevent one `explain` call from consuming hundreds of thousands of tokens while retaining enough evidence for an agent to cite a claim correctly and enough metadata to retrieve any omitted source text through the same tool.

The design treats response size as a **server policy**, not as a promise about model context windows or token counts.

## Verified current behavior

The current implementation has four independent sources of unbounded output:

1. `memory/explanation.rs` copies the full `episode.content` into `citation_context` for every explained item.
2. When an input item has empty `content`, the same full episode body is copied into `content`.
3. `all_sources` stores full `episode_content` for the direct episode and every linked episode. Repeated references can repeat the same body.
4. `compact=true` omits `quote`, but does not bound `content`, `citation_context`, `provenance`, `all_sources`, caller-supplied strings, or JSON escaping overhead.

The current tool adapter returns a `ToolResponse<Value>` envelope, but it has no final response-size guard. The MCP handler only deserializes parameters and delegates; it does not construct the internal `ExplainRequest`.

The current compact decision is ADR-0022. It decided that `compact=true` is the default and that `quote` is omitted because observed quotes duplicated `content`. The July 30 token-efficient-response spec separately preserved full `citation_context` and populated `all_sources.episode_content`. This proposal supersedes the latter policy and amends, but does not replace, ADR-0022's compact default.

## Goals

- Enforce a hard **32 KiB limit on the serialized `ToolResponse<Value>`** returned by `explain`, in compact and verbose modes.
- Preserve a complete, verbatim caller-supplied quote when it fits and can be verified against the source.
- Return bounded, verbatim source evidence rather than generated summaries.
- Make omitted or shortened data explicit through machine-readable reason codes and actionable `guidance`.
- Make complete source text retrievable in contiguous, Unicode-scalar-safe windows through the existing `explain` tool.
- Preserve provenance relationships while removing duplicated full source bodies.
- Keep the eight-tool MCP surface frozen; add no tool and no dependency.

## Non-goals

- Summarize, paraphrase, or synthesize evidence.
- Guarantee a model-independent token count. The server enforces serialized UTF-8 bytes, not tokens.
- Promise that the MCP wire frame, RMCP wrapper, or JSON-RPC framing is at most 32 KiB. The limit applies to the tool-returned envelope measured before transport framing.
- Prevent a caller from making many bounded calls and accumulating the results in its own context.
- Change retrieval, ranking, extraction, fact invalidation, or fact-access tracking.
- Change MCP protocol semantics. The continuation metadata in this design is an application convention on an existing tool, not MCP cursor pagination.
- Add a new downstream answer-quality benchmark in this change. The deterministic eval added here proves evidence fidelity and boundedness, not model answer quality.

## Approaches considered

1. **Omit full-text fields in compact mode only.** Rejected: `compact=false` remains unbounded, and compact callers lose access to evidence without a deliberate recovery path.
2. **Move source reads to an MCP resource or MCP Apps-only chunked tool.** Rejected as the baseline: the app feature is optional, the public tool surface is frozen, and a separate read mechanism duplicates the existing `explain` capability.
3. **Bounded windows plus progressive disclosure in `explain` — selected.** The memory capability owns quote, source-window, and provenance semantics. The tool adapter owns final envelope budgeting. The caller can read any source in explicit contiguous windows without adding a tool.

## Normative terms

- **Unicode scalar value:** one Rust `char`; one Unicode scalar value. Rust `str::len()` is a byte count, not a scalar count.
- **Scalar-safe slice:** a substring selected by Unicode scalar offsets. It may split a grapheme cluster, but it never splits a UTF-8 code point and preserves the original scalar sequence when windows are concatenated.
- **Complete quote:** the entire caller-supplied quote is present in the `quote` field. A prefix of a quote is never serialized as `quote`.
- **Verified quote:** the entire caller-supplied quote occurs verbatim in the resolved source body.
- **Default excerpt:** a quote-anchored or fallback bounded source window. It is not a reconstruction sequence.
- **Explicit source read:** a contiguous range selected with `source_offset` and optional `source_limit_scalars`.
- **Envelope:** the serialized `ToolResponse<Value>` returned by `tools::explain`. The 32 KiB limit is measured on this value.

## Request contract

The existing `explain` tool gains four flat, optional request fields:

| Field | Type | Unit and meaning |
|---|---|---|
| `source_offset` | integer | Zero-based Unicode scalar offset into the selected source episode. |
| `source_limit_scalars` | integer | Maximum Unicode scalar values in an explicit source read. |
| `provenance_offset` | integer | Zero-based index into the selected item's final ordered provenance list. |
| `item_offset` | integer | Zero-based index into the submitted `context_items` array. |

Rules:

1. `source_limit_scalars` requires `source_offset`; it is not a way to change the default excerpt size.
2. `source_offset`, `source_limit_scalars`, or `provenance_offset` require exactly one selected context item. This keeps every continuation unambiguous.
3. `item_offset` may select zero or one item from the submitted array. It is applied before the one-item continuation check.
4. `item_offset == context_items.len()` is a valid empty terminal selection. A larger offset is a validation error.
5. `source_offset == source_length` is a valid empty terminal source window. A larger offset is a validation error.
6. `provenance_offset == provenance_total_count` is a valid empty terminal provenance page. A larger offset is a validation error.
7. An explicit `source_limit_scalars == 0` is valid only for an empty source or an offset at the source end. Otherwise it is rejected because it would produce a non-progressing continuation.
8. An omitted `source_limit_scalars` in explicit source-read mode defaults to **8,000 Unicode scalar values**. Larger requested values are rejected rather than silently clamped, so the caller never mistakes a clamp for the requested range.
9. The default excerpt target is **1,200 Unicode scalar values per item**. It is a target, not a guarantee: the final envelope budget may return less.
10. The default provenance page size is **20 records per item**. Larger provenance lists are paged, not silently clipped.

These are local policy values, not MCP requirements.

## Response contract

### Envelope

The existing envelope fields remain:

- `status`: `"success"` or `"partial"`;
- `result`: array of explained items;
- `guidance`: actionable next step;
- `has_more`, `total_count`, `next_offset`: item-list continuation metadata.

For `explain`, `total_count` is the number of items in the original submitted `context_items` array, not only the number returned. `next_offset` is the index of the first unreturned submitted item, or `null` at the end. It never refers to a source scalar offset or provenance index.

The envelope gains an optional `partial_reasons` array. It contains unique reason codes in a stable order. `partial_reasons` is omitted when empty.

### Explained item

Each item retains the existing fields and adds explicit citation metadata:

| Field | Meaning |
|---|---|
| `citation_context_mode` | `"quote_anchored"`, `"source_window"`, or `"none"`. |
| `citation_context_start_offset` | Unicode scalar offset of the first returned source scalar. |
| `citation_context_scalars_returned` | Number of source scalars actually returned. |
| `citation_context_scalars_total` | Total source scalar count. |
| `citation_context_next_offset` | Next contiguous scalar offset, or `null` at source end. |
| `citation_context_exact` | `true` when the returned context is verbatim source text. |
| `citation_context_covers_entire_source` | `true` only when the returned range starts at zero and covers the full source. |
| `quote_supplied` | The caller supplied a non-empty quote. |
| `quote_returned` | The `quote` field contains text. |
| `quote_complete` | The `quote` field contains the entire supplied quote. |
| `quote_verified` | The entire supplied quote was found verbatim in the resolved source. |
| `quote_anchored` | The default excerpt was positioned using the quote's source location. |
| `quote_contained_in_window` | The entire quote lies inside `citation_context`. |
| `content_truncated` | `content` was shortened by the final reducer. |
| `provenance_total_count` | Number of records in the final ordered provenance list. |
| `provenance_next_offset` | Next provenance index, or `null` at the end. |
| `partial_reasons` | Item-local reason codes. |

`quote` is never serialized as a partial quote. It is either the complete supplied quote or absent. This is why `quote_returned` and `quote_complete` are separate concepts even though the current reducer normally gives them the same value.

### Quote-state truth table

| Situation | supplied | returned | complete | verified | anchored | contained in window | Partial reason |
|---|---:|---:|---:|---:|---:|---:|---|
| No quote supplied | false | false | false | false | false | false | none |
| Quote supplied, source found, whole quote fits | true | true | true | true | true when window is quote-positioned | true only if window covers it | none unless another field is partial |
| Quote supplied, source found, window does not cover whole quote | true | true | true | true | true | false | none if quote itself remains complete |
| Quote supplied, source found, quote omitted by budget | true | false | false | true | false | false | `quote_incomplete`, `response_budget_applied` |
| Quote supplied, source unavailable | true | true if it fits | true | false | false | false | `quote_unverified` |
| Quote supplied, quote not found in source | true | true if it fits | true | false | false | false | `quote_unverified` |

A quote equal to `content` is still returned in compact mode under this proposal. The prior assumption that quotes always duplicate content is not a safe basis for omitting citation evidence.

### Source-window modes

#### Default excerpt

When `source_offset` is absent:

1. Resolve the source episode.
2. If a caller quote occurs verbatim in the source, center a bounded excerpt around its first occurrence.
3. Otherwise, return the first bounded source excerpt.
4. Never copy the full source into `citation_context`.
5. Do not populate `citation_context_next_offset`: a quote-anchored excerpt is not a reconstruction sequence and may omit a prefix.
6. If full reconstruction is needed, guidance identifies an explicit read starting at `source_offset=0`.

If the caller did not supply `content`, the implementation must not fall back to the full episode body. `content` remains empty; the bounded source evidence is represented by `citation_context`.

#### Explicit source read

When `source_offset` is present:

1. Return the contiguous scalar range beginning at `source_offset`.
2. Return no more than the effective limit of 8,000 scalars.
3. Set `citation_context_mode="source_window"`.
4. Set `citation_context_next_offset` to the next unreturned scalar offset, or `null` at source end.
5. A read that does not reach source end is partial with `source_window_incomplete`.
6. Concatenating successive explicit reads from offset zero reconstructs the original source string exactly.

The explicit read does not depend on quote location. A supplied quote is still returned and verified independently.

### Provenance

`all_sources` becomes a metadata-only page:

- `episode_content` becomes optional and is `None` for `explain` output;
- `episode_id`, `episode_t_ref`, `relationship`, and `entity_path` remain available;
- the logical provenance edge is the tuple `(episode_id, relationship, entity_path)`;
- exact duplicate edges are removed;
- the same episode may still appear more than once when it has distinct relationships or entity paths, so deduplication does not discard provenance;
- the final list is ordered deterministically: direct records first, then linked records by `episode_t_ref` descending, then `episode_id` ascending, then `entity_path` ascending;
- `provenance_total_count` and `provenance_next_offset` refer to this final ordered list.

This preserves the existing `ProvenanceSource` shape and avoids introducing a second relationship representation.

### Partial semantics

`status="partial"` means that requested evidence was omitted, shortened, could not be verified, or has a continuation. It does not merely mean that the default excerpt is shorter than the source: the default excerpt is an intentional bounded mode and is identified by `citation_context_mode`.

Reason codes:

| Code | Meaning |
|---|---|
| `quote_unverified` | A supplied quote could not be verified against the resolved source. |
| `quote_incomplete` | The supplied quote was not returned in full. |
| `quote_omitted_by_budget` | The quote was removed specifically by the final envelope reducer. |
| `citation_window_incomplete` | The citation context did not satisfy its declared range or anchor contract. |
| `source_window_incomplete` | An explicit source read ended before source end. |
| `provenance_truncated` | More provenance records exist than the returned page contains. |
| `provenance_omitted_by_budget` | Provenance metadata was removed by the final reducer. |
| `content_truncated` | `content` was shortened by the final reducer. |
| `items_omitted` | One or more submitted items did not fit. |
| `response_budget_applied` | The final 32 KiB reducer changed the candidate response. |

Envelope `partial_reasons` is the stable union of item-local and envelope-level reasons. A `partial` response always has at least one reason code.

## Hard size limit and guaranteed reduction

### Measurement boundary

The limit is:

> `serde_json::to_vec(&ToolResponse<Value>).len() <= 32 * 1024`

The measurement includes the complete tool-returned envelope: status, result, guidance, pagination metadata, and partial reasons. It excludes RMCP and JSON-RPC framing.

This is a local server policy. It is not an MCP guarantee and not a token estimate.

### Reducer requirements

The final reducer must guarantee the limit for every successful `explain` response, including JSON-escaping-heavy content and arbitrarily large caller-supplied metadata.

Required behavior:

1. Measure the complete candidate envelope after serialization, not only `result`.
2. If it exceeds the limit, remove optional diagnostic fields first.
3. Preserve a complete exact quote whenever it fits. Never serialize a partial quote.
4. Reduce optional evidence in a deterministic order: verbose diagnostics, graph insights, provenance detail, citation context, then oversized `content`.
5. When reducing `citation_context`, update `citation_context_scalars_returned` and `citation_context_next_offset` atomically and add `citation_window_incomplete` plus `response_budget_applied`.
6. When reducing `content`, set `content_truncated=true` and add `content_truncated` plus `response_budget_applied`.
7. Omit later items before destroying earlier exact evidence. Set `has_more=true`, `total_count` to the original submitted item count, and `next_offset` to the first omitted item index. Add `items_omitted`.
8. If one item still cannot fit, remove its optional evidence and oversized content. The item may remain with empty `content` and metadata-only continuation fields.
9. As a final fallback, return a minimal envelope with an empty result, `status="partial"`, bounded constant guidance, `has_more=true`, the original `total_count`, and the current `next_offset`. This fallback must fit by construction.
10. Required identifiers are bounded during request validation so the minimal fallback cannot itself exceed the cap.

The reducer is deterministic for a given candidate envelope and must not depend on wall-clock time or hash iteration order.

## Ownership and architecture

```text
MCP handler
  -> tools/explain.rs                    transport adapter
       -> memory/api.rs                  memory use-case seam
            -> memory capability          quote/window/provenance semantics
       -> explain envelope reducer       final byte-budget enforcement
```

Responsibilities:

- `mcp/handlers.rs`: expose the schema and delegate. It does not construct `ExplainRequest` and owns no citation policy.
- `tools/params.rs`: flat request schema and descriptions.
- `tools/explain.rs`: parse input, select items, construct `ExplainRequest`, invoke the capability, and construct the envelope.
- `memory/api.rs`: public memory use-case seam through `ExplanationPort` and `explain_context`.
- `memory/capabilities/explain.rs` and `memory/explanation.rs`: source resolution, quote verification, source-window construction, provenance assembly, and citation metadata.
- `service/memory_container_shims/memory_capabilities_explain.rs`: container-shaped adapter only. It contains no citation or pagination policy.
- `tools/response.rs`: generic envelope constructors, including a production partial constructor with reason codes.

This separation gives the memory context locality over evidence semantics while keeping transport-specific byte budgeting in the tool adapter. It avoids duplicating citation policy across the handler, capability, and shim, and avoids introducing another shared container dependency.

## Compatibility and migration

1. **Eight-tool surface:** unchanged. Only parameters on `explain` are added.
2. **Compact quote exception:** ADR-0022 is amended to state that `explain` keeps a complete bounded exact quote in compact mode. Other compact omissions remain unchanged.
3. **Full-body policy:** The newly allocated citation-output ADR will supersede the July 30 decision to inline full `citation_context` and populated `all_sources.episode_content`.
4. **Verbose compatibility break:** `compact=false` remains diagnostically richer than compact mode but is now bounded. It no longer means unbounded or full-body output.
5. **`ProvenanceSource.episode_content`:** changes from required `String` to optional `String`. Existing constructors must be updated explicitly; no silent full-body default is allowed.
6. **Continuation convention:** `ToolResponse.next_offset` remains an item index for `explain`. Source and provenance offsets use their own named fields. No overlapping offset names are introduced.

## Acceptance criteria

1. Every successful compact and verbose `explain` response serializes to at most 32 KiB for multi-megabyte direct and linked sources, repeated references, JSON-escaping-heavy text, and oversized caller metadata.
2. The reducer never exceeds the cap, never serializes a partial quote as `quote`, and updates every affected range/completeness field atomically.
3. When a supplied quote is verified and fits, it is present in compact and verbose responses and remains byte-for-byte identical to the supplied quote.
4. Successive explicit source reads from offset zero reconstruct the original source string exactly, without skipping or duplicating Unicode scalar values. Grapheme clusters may be split; this is explicitly not a grapheme guarantee.
5. Default quote-anchored excerpts are not represented as reconstruction sequences; `citation_context_next_offset` is absent for that mode.
6. Terminal and invalid boundaries behave as specified for item, source, and provenance offsets, including empty source, empty terminal windows, zero limit, and out-of-range offsets.
7. Provenance records preserve every distinct `(episode_id, relationship, entity_path)` edge, contain no full episode bodies, have deterministic ordering, and expose correct totals and continuations.
8. Compact and verbose responses use explicit partial reason codes; `partial` never appears without at least one reason.
9. The deterministic eval measures the actual `ToolResponse` envelope, proves boundedness and exact source reconstruction, and does not claim downstream answer quality.
10. A canonical public-tool-name snapshot proves that exactly these eight tools remain registered: `ingest`, `explain`, `extract`, `resolve`, `invalidate`, `open_app`, `app_command`, `assemble_context`.
11. The memory capability owns evidence semantics; the tool adapter owns only envelope budgeting; the MCP handler and container shim contain no duplicated citation policy.
12. Existing compact defaults and unrelated compact serialization behavior remain intact except for the explicit `explain` quote exception.

## Research

### Facts

- The MCP tools specification defines `tools/call` but no response-size cap. [MCP Tools, 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/tools)
- MCP pagination is cursor-based and applies to list operations such as `tools/list` and `resources/list`; it is not a `tools/call` response-size mechanism. [MCP Pagination, 2025-11-25](https://modelcontextprotocol.io/specification/2025-11-25/server/utilities/pagination)
- Anthropic's tool guidance recommends pagination, range selection, filtering or truncation, sensible defaults, and actionable guidance for large results. [Writing effective tools for agents](https://www.anthropic.com/research/writing-tools-for-agents)
- Anthropic's MCP builder reference recommends explicit `has_more`, `next_offset`, and `total_count`, plus graceful response-size limits. [MCP builder best practices](https://github.com/anthropics/skills/blob/f5ce30fe5fa486f822065270f7f66b642e770f8a/mcp-builder/reference/mcp_best_practices.md)
- MCP Apps documents chunked reads as an application pattern for keeping large content out of model context; it is not mandatory for ordinary MCP tools. [MCP Apps patterns](https://apps.extensions.modelcontextprotocol.io/api/documents/patterns.html)
- Rust `str::len()` returns UTF-8 bytes; Rust `char` is a Unicode scalar value. Scalar slicing can split a grapheme cluster. [Rust `str` documentation](https://doc.rust-lang.org/1.99.0/std/primitive.str.html)
- Offset pagination can skip or duplicate records when the underlying collection changes, so it is most appropriate for small, mostly stable collections and requires documented ordering and continuation semantics. [API pagination design patterns](https://wso2.com/api-platform/learn/api-pagination-design-patterns)

### Inferences

- A server-local byte cap is appropriate because the protocol does not supply one and the project must protect both stdio and HTTP clients consistently.
- Offset pagination is acceptable for the bounded, derived provenance graph, provided the final order is deterministic and the contract explicitly avoids snapshot guarantees.
- Semantic reduction plus a minimal-envelope fallback is safer than attempting to predict token counts or only truncating one large string field.

### Assumptions

- 32 KiB, 1,200 scalars, 8,000 scalars, and 20 provenance records are conservative starting policies. They require telemetry and eval review before future tuning.
- Most consumers need a compact exact quote more often than a full surrounding episode body.
- Explicit contiguous reads are acceptable for the relatively rare case where complete source reconstruction is required.

### Gaps

- No model-independent, reliable byte-to-token conversion exists; the server intentionally measures bytes.
- No current protocol mechanism guarantees a maximum `tools/call` payload across implementations.
- The deterministic eval added by this design cannot prove downstream answer quality; that requires a separate model-based evaluation and is out of scope.
