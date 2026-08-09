# ADR-0037: NER Extractor Evaluation Comparison

> Status: Accepted
> Date: 2026-08-08
> Related: ADR-0025 (suite metric provenance), ADR-0036 (unified NER_EXTRACTOR)

## Context

The closed `NER_EXTRACTOR` catalog spans five backends with very different profiles:
offline rules (anno/regex), CPU ONNX (anno-onnx), and two native Candle GLiNERs
(classic and VAGO LFM2). Users need a way to compare their quality and latency on
the same inputs to choose the right backend for a scenario. The eval harness had
latency benches for GLiNER/VAGO only, and no quality suite parameterized over
extractors.

## Decision

Add a manual-only evaluation workflow, in `eval-harness`, covering every backend:

1. A shared RU/EN/mixed quality corpus (`evals/corpora/ner/ner_quality.json`) with
   hand-annotated spans/labels, structurally validated offline. Six cases reuse the
   VAGO release-parity corpus verbatim (pinned by a consistency test).
2. One `NerQualitySuite` per extractor (`ner-quality-anno|regex|anno-onnx|gliner|vago`)
   that builds the extractor through the production `create_entity_extractor` path,
   fixture-gated (missing checkpoint => explicit invalid cases, never a download).
   Mention-level precision/recall/F1 use the existing `ClassificationReducer`;
   typed match is a per-case diagnostic.
3. Full CPU latency bench coverage (`ner_cpu.rs`) with cold-start reporting.
4. A dedicated `ner_quality` profile; the suites are excluded from `pr`/`release`/
   `nightly` profiles because CI has no model checkpoints and any invalid outcome
   invalidates a run.

## Consequences

Users get a comparable quality + latency matrix per extractor. Model-backed suites
require locally prepared checkpoints (gitignored fixtures). Suite ids are stable and
mirror the `NER_EXTRACTOR` selectors. Adding a future backend means: extend the
corpus if needed, add one suite registration, and one bench.
