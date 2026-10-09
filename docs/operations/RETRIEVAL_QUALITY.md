# Retrieval quality contract

`assemble_context` returns a bounded set of evidence for the caller to assess.
The selected IDs, source provenance, and current access rules define the
response. A successful response does not prove that the set is complete or
that a compound query has one approved answer.

## Response fields

`confidence` is the fact's time-decayed confidence. `relevance` is the final
normalized ranking score for this response item. The two values answer
different questions. In verbose responses, a numeric `relevance=` label in
`rationale` matches the returned `relevance` after the same two-decimal
rounding.

`retrieval_evidence` is optional and appears only in verbose responses. It
contains the admission-policy version and basis, the raw semantic cosine when
the semantic channel contributed that evidence, and graph-anchor kind and hop
count when graph evidence contributed. `semantic_similarity` is the provider
and index score in the cosine range; it is not confidence, normalized
relevance, or a probability. The field describes why this item was admitted,
not whether the returned set covers every part of the query.

Compact mode omits `retrieval_evidence` and other verbose-only detail while
preserving the selected fact IDs. Use `explain` for provenance-ready citations
for selected items. `explain` rechecks access policy for each fact and source
episode before returning lineage.

When no sufficient matching item is found, guidance is:

> No sufficient matching context was found. Refine the query or verify primary sources.

When items are present, guidance points to `explain` for their citations. It
does not assert completeness or approval.

Storage/index failures propagate as tool errors; they are not represented as a
successful empty result. An embedding-provider outage is a separate degraded
mode: lexical retrieval remains available and the outage is logged at WARN.

## Verification

The wire contract and score semantics have focused regression coverage:

```sh
cargo test -p memory_mcp --locked --features test-fixtures --test assemble_context_wire_contract -- --test-threads=1
cargo test -p memory_mcp --locked --features test-fixtures,eval-support --test assemble_context_semantic_quality -- --test-threads=1
cargo test -p memory_mcp --locked --features test-fixtures,eval-support,streamable-http --lib assembled_context_item
```

Component and embedded-store tests establish the mechanics of retrieval,
serialization, access filtering, and error propagation. They are not a
source-reviewed workload acceptance result. A release-quality claim still
requires the frozen native corpus, independent labels, pinned provider/index
signatures, and the fail-closed eval-harness run described in the retrieval
quality plan.
