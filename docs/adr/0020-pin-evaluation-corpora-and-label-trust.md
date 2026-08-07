# ADR-0020: Pin Evaluation Corpora and Separate Label Trust

> Status: Accepted (2026-07-28; implemented by the eval-harness corpus manifests, pinned revisions, and trust-aware release gates)
> Related: ADR-0019 (profile-driven truthful evaluation)

## Context

External evaluation currently relies on datasets fetched from mutable branch
URLs without a cryptographic identity. Evaluation may download data at run
time, samples may be selected by file prefix, and heuristic labels can be
aggregated with stronger ground truth.

These behaviors prevent exact reproduction, allow corpus drift to masquerade as
a product regression or improvement, and make release metrics depend on
unreviewed relevance assumptions.

## Decision

Separate corpus preparation from evaluation execution.

An explicit preparation command fetches an immutable declared revision and
produces a manifest containing:

- source URL;
- revision;
- SHA-256 digest;
- license;
- byte size;
- case count;
- corpus adapter version.

Evaluation is offline with respect to corpus acquisition. It validates the
prepared files against the manifest and never downloads or modifies them.
Missing data, a digest mismatch, a count mismatch, or an incompatible adapter
version makes the affected suite invalid.

Sampling and sharding use a stable hash of corpus identity and case ID. Samples
are explicitly stratified, and every artifact records selected IDs, strata, and
coverage. Prefix sampling is prohibited.

Every expected label carries one trust level:

1. `official` for dataset-provided IDs or labels;
2. `reviewed` for project-maintained human-reviewed mappings with provenance;
3. `weak` for heuristic or inferred labels.

Only official and reviewed labels may contribute to a release gate. Weak labels
remain visible as a separate diagnostic slice.

## Consequences

- Any published result can be tied to exact corpus bytes and adapter behavior.
- Evaluation runs no longer depend on network availability or mutable upstream
  branches.
- Corpus preparation becomes an explicit operational step and requires storage
  for prepared data.
- Dataset revisions and reviewed-label corrections become reviewable changes
  with before/after artifacts.
- Stable stratification gives repeatable PR samples while release shards cover
  the complete declared population.
- Weak supervision remains useful for exploration but cannot inflate the
  release result.

## Alternatives Considered

### Download the latest upstream data during each run

Rejected because reproducibility and regression attribution would depend on
mutable external state.

### Commit every external corpus into the repository

Rejected because large datasets, licensing constraints, and revision cadence
make the source repository an unsuitable distribution mechanism.

### Treat all relevance mappings as equivalent

Rejected because heuristic matches do not provide the same evidence as
official or reviewed labels.

### Use prefix samples

Rejected because source ordering can overrepresent common families and omit
rare cases. Stable stratification makes the selection explicit and repeatable.

## Verification

The decision is satisfied when:

- a prepared corpus with an incorrect digest makes the suite invalid;
- evaluation performs no corpus network access or mutation;
- repeated sampling with the same corpus identity produces the same case IDs;
- shard union equals the declared complete population without duplicates;
- artifacts report manifest identity, selected IDs, coverage, and label trust;
- weak-label metrics are excluded from release-gate aggregation.

