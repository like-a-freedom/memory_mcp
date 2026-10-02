# ADR-0068: The extractor fingerprint is an opaque revision token

- Status: accepted
- Date: 2026-10-02
- Related: ADR-0067, ADR-0058

## Context

Seven `EntityExtractor` adapters exist: a lightweight rule-based one, a GLiNER
backend, and the variants that differ in device, threshold or label set.
CONTEXT.md says the Entity Extractor is a capability whose seam "does not
expose model architecture, checkpoint format, or runtime details", and the
plan that established that sentence made it a constraint.

`ExtractorFingerprint` made the constraint false. It carried
`revision_status: Option<RevisionStatus>`, `validation_status:
Option<ValidationStatus>` and `effective_device: Option<String>` — two types
from `embedding::model_artifacts` and a runtime detail — all in the
capability's public interface. A consumer holding a fingerprint could branch
on whether a checkpoint had been validated, or on which device an extractor
had actually landed on. Neither is knowledge's business; both are the
model-artifact module's.

Thirty-eight references to `model_artifacts` sat inside `knowledge/`,
including the candidate-versus-known-good promotion state machine
(`gliner.rs:1668-1709`) and safetensors metadata inference
(`gliner.rs:417,447,563`). Some of those are legitimately knowledge's — it
decides whether to promote a candidate. What is not legitimate is the type
crossing the capability's public seam.

## Decision

`ExtractorFingerprint` becomes an opaque newtype over a token.

```rust
/// Identifies the Model Checkpoint an extractor is running, as an opaque
/// token. A caller compares tokens for equality and nothing else; the
/// status, the device, and the artifact identity live in the model-artifact
/// module, which is the only place that can interpret them.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ExtractorFingerprint(String);
```

The formatting moves into `embedding/model_artifacts` as
`revision_token(&self) -> String`, and each adapter calls it. The token's
format is `<repository>@<revision>:<revision_status>`, pinned by a test so a
change to it is a deliberate cache invalidation rather than an accident.

## Consequences

A consumer that wanted to branch on `revision_status` must call the
model-artifact module, which is the only place that can interpret a status.
That is the point: the answer now has to come from the module that owns the
question.

`knowledge/` still imports `model_artifacts` internally — it has to, to
promote a candidate and to infer safetensors metadata. What changes is that
the capability's *public interface* names no model-artifact type, which is
what `knowledge_does_not_name_a_model_artifact_type` asserts.

## Alternatives considered

(a) **Keep the rich fingerprint.** Rejected. It makes the capability's
interface a function of the checkpoint format, which is exactly what
CONTEXT.md forbids. Adding a fourth status variant would become a breaking
change to a knowledge interface.

(b) **Move the whole `EntityExtractor` trait into `embedding`.** Rejected.
Extraction is a knowledge capability — it produces entity candidates — and
five of the seven backends are not embedding providers at all. Moving the
trait would give the embedding context a say in entity semantics, which is
the same mistake ADR-0058 was written to stop.

(c) **Keep the fields but mark them `pub(crate)`.** Rejected. The type is in
a `pub` trait signature, so the fields have to be public for the trait to be
implementable outside the module — and the eval harness does implement it.
