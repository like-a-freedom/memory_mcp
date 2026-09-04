# NER quality baseline review

This is a reviewed clean-run record for the committed `ner` fixture on
2026-09-05. It is diagnostic evidence for the five backend profiles, not a
claim that the models are interchangeable or production-optimal.

Command:

```text
make eval-ner-quality
```

Observed entity-mention F1:

| Backend | F1 | Profile floor | Rationale |
|---|---:|---:|---|
| Anno | 0.7473 | 0.70 | floor is below the reviewed clean run with margin for fixture/runtime noise |
| Regex | 0.7447 | 0.70 | same conservative floor policy as Anno |
| Anno-ONNX | 0.2185 | 0.15 | lower floor reflects the known weaker ONNX fixture while still catching collapse |
| GLiNER | 0.9184 | 0.85 | floor catches material model/fixture regressions |
| VAGO | 0.9302 | 0.85 | floor catches material model/fixture regressions |

The evaluator reports valid but imperfect extraction as a passed execution
case; precision/recall/F1 are the quality evidence. A missing model fixture or
runtime error is not a pass and must be reported as skipped/invalid according
to the platform prerequisites.
