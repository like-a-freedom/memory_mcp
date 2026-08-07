# ADR 0029: Registry of pluggable NER backends

## Status

Accepted (implemented in `crates/memory-mcp/src/service/entity_extraction.rs`).
Amended by ADR-0036: the registry seam remains, while public provider/model configuration is replaced by typed extractor selection.

## Context

Entity extraction used a single `create_entity_extractor` factory with a
hard-coded `match` on `NerProviderKind`. The GLiNER model-loading path
(config validation, model-dir resolution, model download/caching, and
`GlinerEntityExtractor::new_with_runtime`) lived in the factory, not in the
GLiNER module — a GLiNER lock-in that leaked Candle/model-loader concerns into
the shared dispatch point and made adding another backend touch multiple files.

## Decision

Each backend module (`regex`, `anno`, `gliner`) owns its construction via a
`pub(crate) fn build(config, data_dir, logger) -> BackendBoxFuture` hook. A
static `backend_registry()` maps each `NerProviderKind` to `{ kind, name,
build }`. `create_entity_extractor` is the single dispatch point: it looks up
the kind and awaits the hook.

- Behavior is unchanged: same defaults, thresholds, env names, error kinds,
  and `provider_name()` strings (`"regex"`, `"anno"`, `"gliner"`, `"llm"`).
- The LLM extractor stays code-injected (`LlmEntityExtractor::new(f)`); it has
  no `NerProviderKind` and no registry entry.

## Adding a new backend

The implemented extension steps below describe the pre-ADR-0036 configuration shape:

1. Create `crates/memory-mcp/src/service/entity_extraction/<name>.rs` implementing `EntityExtractor` and exposing a backend build hook.
2. Add the backend to the single registry dispatch table.
3. Update the registry-size and dispatch tests.

ADR-0036 removes the `NerProviderKind`/`NER_PROVIDER` extension step. New work must instead add one closed-catalog extractor selection and its typed configuration variant. A model-backed extractor also declares backend-owned artifact requirements consumed by the shared artifact lifecycle. No other dispatch sites should be introduced.

## Consequences

- GLiNER-specific loading is fully contained in `gliner.rs`; the factory no
  longer imports `model_loader` or Candle types.
- Adding/removing a provider is a localized, single-table edit.
