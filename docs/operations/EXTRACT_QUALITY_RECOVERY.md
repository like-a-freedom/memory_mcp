# Extract quality: historical recovery assessment

Status: assessment only. Do not execute a historical repair from this document.
The local implementation and quality gates are complete. Production deployment
and selective historical repair remain separate operational decisions.

## Record inventory

- Episodes retain the source text and source identity. Extracted facts refer
  to the source episode and carry provenance and valid/ingested times.
- Entity extraction appends `entity_extraction_projection` rows containing
  episode, extractor fingerprint and resolved entity IDs. Extracted
  `mentioned_in` edges connect entities to episodes; `involved_in` edges
  connect entities to facts.
- Claims carry source fact and episode IDs, source lineage and a projection
  fingerprint. Claim relations refer to both facts and have an invalidation
  transaction timestamp. Jobs track projection and reconciliation work.

## Available repair boundary

The public `invalidate` capability targets a fact. When its claim pipeline is
wired, it closes the fact's derived claims and relations as well as the fact,
and invalidates the context cache. Therefore it is not a way to discard only
bad derived claims while retaining a valid source fact as active. The normal
extraction projection is append-only; a new projection does not establish that
old entities, edges, claims, relations or jobs have been retired. Repeating
`extract` is not a complete historical cleanup procedure.

No supported public operation has been established here for selectively
retiring an erroneous automatic entity link or claim relation while preserving
the underlying fact. Do not issue direct writes or delete historical rows to
work around that gap. A separate reviewed recovery design and, if necessary,
an ADR or migration are required before execution.

## Preparation for a separate recovery decision

1. Pin the deployment, active namespace, time window, backup and extraction
   and policy fingerprints. Produce an auditable, read-only inventory of
   affected episode/fact IDs and their entity, edge, claim, relation, job and
   projection dependencies; keep source text out of shared logs.
2. Sample the inventory against source episodes, separating incorrect derived
   state from correct facts and valid relations. Record dry-run counts by type,
   source, fingerprint and temporal state; do not infer all rows sharing a
   heading are wrong.
3. Validate a proposed append-only closure/reprojection mechanism on an
   isolated snapshot, including retries, lineage, policy isolation, cache
   invalidation and interrupted execution. Reconcile before/after counts and
   verify raw facts remain retrievable.
4. Compare `assemble_context`, genuine and false contradiction behavior, and
   extract latency on a fixed corpus. Obtain the applicable review for the
   recovery mechanism and deployment before any production mutation.

The production baseline and its measurement limits are recorded in
`docs/performance/EXTRACT_QUALITY_COST_BASELINE.md`.
