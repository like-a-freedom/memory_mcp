# Historical identity case sensitivity

The public temporal read reproduced a wrong-source admission. The query
`What capacity was recorded for COPPER WAREHOUSE?` returned the records for
both Copper Warehouse and Copper Warehouse West. The public regression
requires exactly the first source and no reconciliation annotation.

The change recognizes uppercase multiword names in mixed-case questions
through the existing explicit-subject parser. The declared subject still
has to match the normalized full name. The speaker-subject inference
continues to use its existing title-case rule. This slice covers the
mixed-case question form. All-caps questions and revisioned alias recall
are outside its coverage.

## Reproduction

Build both product revisions with
`cargo build -p memory_mcp --locked --bin memory_mcp --features eval-support`.
Preserve both binaries under distinct absolute paths and record their
SHA-256 sums. Use Rust 1.100.0 on aarch64-apple-darwin to reproduce the
native run.

Capture each binary twice with a fresh absolute artifact directory.

```sh
MEMORY_DECISIONS_SERVER_BIN="$binary" \
MEMORY_DECISIONS_ARTIFACT_ROOT="$capture" \
MEMORY_DECISIONS_PRODUCT_REVISION="$revision" \
target/debug/memory-eval run \
  --profile evals/profiles/memory_decisions_historical_identity.json \
  --artifact "$capture/run.json"
```

Run `compare-decision-replays` on each pair, verify the binary identities
against the captured hashes, and run `accept-memory-candidate` with the
four indexes and the [registered plan](../../corpora/memory-decisions-historical-identity/acceptance.json).
The gain threshold is 3333 basis points with at most five items per read
and two ordinary control cases. The cohort is synthetic and small.

## Historical repair decision

The two sources in this reproduction form two correctly attributed
assertions. Each ordinary control retrieves only the source for its named
subject. The wrong-source result occurs at read admission. This case
supplies no affected persisted records for repair. Historical repair
remains gated on a separate reproduction of durable misattribution with
revision history and an identified record set from an approved fixture.

## Acceptance result

Both repeated replays matched. The [verdict](verdict.json) records a gain
from 2/3 to 3/3 families and one eliminated incumbent safety finding
(uppercase: forbidden source). Both ordinary cases stayed green. The
candidate had zero hard failures and zero losses. The uppercase read
returned one item instead of two. Its compact structured JSON shrank from
1960 to 1313 bytes. The byte counts are descriptive. Thirty focused Rust
tests and the required workspace Clippy check passed.
