# Selective recovery of legacy derived evidence

**Status** · Proposed for review. The synthetic inventory supports a
selective recovery design. Production eligibility requires its own inventory,
restore rehearsal and approval.

## Reproduced scope

A disposable embedded store was formed on native Darwin arm64 using
`memory_mcp` at `8d1484400ccbf47e8b68624a51e68f6c1ebc1348`. The public
`ingest` and `extract` calls used evidence mode and no embeddings.
Both documents had reference time `2026-11-01T00:00:00Z`. They described
separate dated records in the following order.

| Source ID | Source episode | Fact | Date and duration |
| --- | --- | --- | --- |
| `workout-oct24` | `episode:ff9b2d1e5310cddb515066bd` | `fact:3e2bf84899d1482a875972cb` | 2026-10-24, 95 |
| `workout-oct29` | `episode:a929385c1b38bf3c057076ee` | `fact:7919df3a5941f038b3e2f3ae` | 2026-10-29, 30 |

The second extraction returned a contradiction between the two facts.
Both direct source assignments were correct. Each `explain` result also
linked the other source through `entity:7a5f1423b4b5112c5ba6fc04`, the
legacy canonical date entity. The claim relation and graph linkage are
distinct derived records and need independent eligibility checks.

The clone opened under current product commit `7689e736` after the
existing SurrealDB 4 pre-start maintenance. Current `assemble_context`
still gave both facts a `conflict` role and `explain` still included the
foreign sources. Fresh extraction on the current binary gave zero
contradictions and one direct citation per fact. A separate rollback
clone reopened with the incumbent and reproduced the ordered fact IDs
and linked sources. The original snapshot retained the same file hashes
through both exercises.

The legacy candidate database reports knowledge revision 0 for the
explicit historical query. Its historical response is partial and
declares the missing legacy revision provenance. The original records
predate the revision ledger. Preserve their ingest order, transaction
timestamps and recovered snapshot identity. Any remediation receives a
new committed revision and must preserve the observed legacy limitation.

To reproduce, build both revisions with the workspace lockfile on native
Apple Silicon. Use a disposable embedded SurrealKV database and disable
embeddings. For each table row above, pass the source ID to `--source-id`
with source type `doc` and submit a document consisting of `Facts:` followed by
`- Historical record: {"date":"2026-10-24","duration":95}` (substitute the
second date and duration for the second document). Extract immediately
after each ingestion. Read `Historical record duration` with budget 5
and `--as-of 2099-01-01T00:00:00Z` on the incumbent. Pass the two items
to `explain`. Shut down before copying the database.
Apply the documented SurrealDB 4 pre-start maintenance to a disposable
copy before opening it with the newer binary. Repeat the read without
`--as-of`,
then request a snapshot with valid time `2026-11-01T00:00:01Z` and
knowledge time `2099-01-01T00:00:00Z`. Reopen a separate unconverted copy
with the incumbent to verify recovery.

The [synthetic replay and inventory](../../../evals/paired/historical-legacy-evidence-2026-11-24/receipt.json)
records the affected claims, relation, graph edge IDs, endpoints and
transaction timestamps. The proposed dry run must establish which of
these derived records qualify for closure.

## Operator dry run

Provide a read-only command through an existing operator transport.
The knowledge owner selects exact rows within the active namespace by
projector fingerprint and IDs. The report contains the namespace,
snapshot and product fingerprints, the revision or legacy marker, the
source episode and fact IDs, the suspect claim, relation and edge IDs,
and the rule and evidence behind each proposed closure. Include separate
totals for eligible rows, rows kept and ambiguous rows. A zero record
inventory returns a successful, empty report. Keep source text in the
source read path and reserve the dry-run artefact for identifiers,
decisions and references.

A reviewed report pins the expected current version or digest of each
target, the corresponding source IDs and a deterministic repair key.
These preconditions must be revalidated immediately before any write.
An inventory mismatch requires a fresh report. A failed provenance or
authorization check ends the operation before mutation.

## Append-only correction

Implement selective repair in the knowledge owner and use the existing
namespace and revision owner. An accepted plan records an append-only
repair event for each explicitly reviewed derived record and publishes
one coherent change at a committed knowledge revision. Use the same
repair key for retries. An identical committed plan returns its original
revision. A reused key with different targets fails before mutation.
Facts, episodes and correct derived records remain retrievable.

Current claim and graph readers apply reviewed suppressions from that
revision forward. The repair event and visibility change must commit
together, with cache invalidation tied to commit. The older content and
audit trail remain readable through the owner. An older revision retains
its prior visibility and partial-completeness semantics. Historical graph
channels omitted today stay outside the historical response. Restoring a
suppressed derivation, if necessary, is a separately reviewed append-only
event with its own revision and provenance. Fact-wide invalidation is
inappropriate for this fixture because both source facts are correct.

## Verification and recovery

- Recreate the two-record fixture via public ingest and extract, then
  compare a fresh current formation with the current product reading the
  restored legacy state. Add separately registered genuine-conflict and
  unrelated-citation controls.
- Freeze preflight and dry-run output before writes. Assert the exact
  affected relation and edge IDs, unchanged direct facts, matching
  public replay before the new revision, and removal of false conflict
  and linkage at the new revision. Assert genuine conflicts and unrelated
  cases remain stable, and assert truthful partial/unknown metadata for
  the unrevisioned history.
- Verify an identical retry, a stale report, concurrent execution and a
  crash before commit. Replay after restart and require stable evidence
  and one committed repair identity.
- Rehearse recovery using a hashed external snapshot. For the reproduced
  case, isolate the database, restore a new copy and reopen it with the
  original engine and binary. Match direct IDs and observed linkage.
  During a reviewed maintenance deployment, define the writer stop point
  and the disposition of writes since backup before restoring. Use the
  existing SurrealDB upgrade and restore instructions for the storage
  conversion. Verify the namespace, tool health and both snapshots before
  admitting writers again.
- Request explicit review before introducing an operator repair
  interface, changing a migration, or running a repair on real data.
  A production dry run requires an approved backup or representative
  window with the exact affected-record inventory.

[SurrealDB upgrade and maintenance](2026-11-16-surrealdb-4-upgrade-design.md) ·
[Existing recovery assessment](../../operations/EXTRACT_QUALITY_RECOVERY.md)
