# ADR 0029: Registry of pluggable NER backends

## Status

Accepted (implemented in `crates/memory-mcp/src/service/entity_extraction.rs`).

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

1. Create `crates/memory-mcp/src/service/entity_extraction/<name>.rs` implementing `EntityExtractor` and exposing `pub(crate) fn build(config: NerConfig, data_dir: String, logger: StdoutLogger) -> BackendBoxFuture`.
2. Add a `NerProviderKind` variant and its `NER_PROVIDER` env alias (`config/ner.rs`).
3. Add one `BackendSpec { kind, name, build }` entry to `backend_registry()`.
4. Update the registry-size test in `entity_extraction.rs::tests`.

No other dispatch sites exist; no framework changes are needed.

## Consequences

- GLiNER-specific loading is fully contained in `gliner.rs`; the factory no
  longer imports `model_loader` or Candle types.
- Adding/removing a provider is a localized, single-table edit.
